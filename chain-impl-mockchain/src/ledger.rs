//! Mockchain ledger. Ledger exists in order to update the
//! current state and verify transactions.

use crate::block::Message;
use crate::fee::LinearFee;
use crate::stake::{DelegationError, DelegationState, StakeDistribution};
use crate::transaction::*;
use crate::value::*;
use crate::{account, certificate, legacy, setting, stake, utxo};
use chain_addr::{Address, Discrimination, Kind};
use chain_core::property;
use std::sync::Arc;

// static parameters, effectively this is constant in the parameter of the blockchain
#[derive(Clone)]
pub struct LedgerStaticParameters {
    pub discrimination: Discrimination,
}

// parameters to validate ledger
#[derive(Clone)]
pub struct LedgerParameters {
    pub fees: LinearFee,
    pub allow_account_creation: bool,
}

/// Overall ledger structure.
///
/// This represent a given state related to utxo/old utxo/accounts/... at a given
/// point in time.
///
/// The ledger can be easily and cheaply cloned despite containing reference
/// to a lot of data (millions of utxos, thousands of accounts, ..)
#[derive(Clone)]
pub struct Ledger {
    pub(crate) utxos: utxo::Ledger<Address>,
    pub(crate) oldutxos: utxo::Ledger<legacy::OldAddress>,
    pub(crate) accounts: account::Ledger,
    pub(crate) settings: setting::Settings,
    pub(crate) delegation: DelegationState,
    pub(crate) static_params: Arc<LedgerStaticParameters>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    NotEnoughSignatures(usize, usize),
    UtxoValueNotMatching(Value, Value),
    UtxoError(utxo::Error),
    UtxoInvalidSignature(UtxoPointer, Output<Address>, Witness),
    OldUtxoInvalidSignature(UtxoPointer, Output<legacy::OldAddress>, Witness),
    OldUtxoInvalidPublicKey(UtxoPointer, Output<legacy::OldAddress>, Witness),
    AccountInvalidSignature(account::Identifier, Witness),
    UtxoInputsTotal(ValueError),
    UtxoOutputsTotal(ValueError),
    Account(account::LedgerError),
    NotBalanced(Value, Value),
    ZeroOutput(Output<Address>),
    Delegation(DelegationError),
    InvalidDiscrimination,
    ExpectingAccountWitness,
    ExpectingUtxoWitness,
}

impl From<utxo::Error> for Error {
    fn from(e: utxo::Error) -> Self {
        Error::UtxoError(e)
    }
}

impl From<account::LedgerError> for Error {
    fn from(e: account::LedgerError) -> Self {
        Error::Account(e)
    }
}

impl From<DelegationError> for Error {
    fn from(e: DelegationError) -> Self {
        Error::Delegation(e)
    }
}

impl Ledger {
    pub fn new(static_parameters: LedgerStaticParameters, settings: setting::Settings) -> Self {
        Ledger {
            utxos: utxo::Ledger::new(),
            oldutxos: utxo::Ledger::new(),
            accounts: account::Ledger::new(),
            settings: settings,
            delegation: DelegationState::new(),
            static_params: Arc::new(static_parameters),
        }
    }

    /// Try to apply messages to a State, and return the new State if succesful
    pub fn apply_block(
        &self,
        ledger_params: &LedgerParameters,
        contents: &[Message],
    ) -> Result<Self, Error> {
        let mut new_ledger = self.clone();

        for content in contents {
            match content {
                Message::OldUtxoDeclaration(_) => unimplemented!(),
                Message::Transaction(authenticated_tx) => {
                    new_ledger = new_ledger.apply_transaction(&authenticated_tx, &ledger_params)?;
                }
                Message::Update(update_proposal) => {
                    new_ledger = new_ledger.apply_update(&update_proposal)?;
                }
                Message::Certificate(authenticated_cert_tx) => {
                    new_ledger =
                        new_ledger.apply_certificate(authenticated_cert_tx, &ledger_params)?;
                }
            }
        }
        Ok(new_ledger)
    }

    pub fn apply_transaction<Extra: property::Serialize>(
        mut self,
        signed_tx: &AuthenticatedTransaction<Address, Extra>,
        dyn_params: &LedgerParameters,
    ) -> Result<Self, Error> {
        let transaction_id = signed_tx.transaction.hash();
        self = internal_apply_transaction(
            self,
            dyn_params,
            &transaction_id,
            &signed_tx.transaction.inputs[..],
            &signed_tx.transaction.outputs[..],
            &signed_tx.witnesses[..],
        )?;
        Ok(self)
    }

    pub fn apply_update(mut self, update: &setting::UpdateProposal) -> Result<Self, Error> {
        self.settings = self.settings.apply(update);
        Ok(self)
    }

    pub fn apply_certificate(
        mut self,
        auth_cert: &AuthenticatedTransaction<Address, certificate::Certificate>,
        dyn_params: &LedgerParameters,
    ) -> Result<Self, Error> {
        self = self.apply_transaction(auth_cert, dyn_params)?;
        self.delegation = self.delegation.apply(&auth_cert.transaction.extra)?;
        Ok(self)
    }

    pub fn get_stake_distribution(&self) -> StakeDistribution {
        stake::get_distribution(&self.delegation, &self.utxos)
    }
}

/// Apply the transaction
fn internal_apply_transaction(
    mut ledger: Ledger,
    dyn_params: &LedgerParameters,
    transaction_id: &TransactionId,
    inputs: &[Input],
    outputs: &[Output<Address>],
    witnesses: &[Witness],
) -> Result<Ledger, Error> {
    assert!(inputs.len() < 255);
    assert!(outputs.len() < 255);
    assert!(witnesses.len() < 255);

    // 1. verify that number of signatures matches number of
    // transactions
    if inputs.len() != witnesses.len() {
        return Err(Error::NotEnoughSignatures(inputs.len(), witnesses.len()));
    }

    // 2. validate inputs of transaction by gathering what we know of it,
    // then verifying the associated witness
    for (input, witness) in inputs.iter().zip(witnesses.iter()) {
        match input.to_enum() {
            InputEnum::UtxoInput(utxo) => {
                ledger = input_utxo_verify(ledger, transaction_id, &utxo, witness)?
            }
            InputEnum::AccountInput(account_id, value) => {
                ledger.accounts = input_account_verify(
                    ledger.accounts,
                    transaction_id,
                    &account_id,
                    value,
                    witness,
                )?
            }
        }
    }

    // 3. verify that transaction sum is zero.
    // TODO: with fees this will change
    let total_input =
        Value::sum(inputs.iter().map(|i| i.value)).map_err(|e| Error::UtxoInputsTotal(e))?;
    let total_output =
        Value::sum(inputs.iter().map(|i| i.value)).map_err(|e| Error::UtxoOutputsTotal(e))?;
    if total_input != total_output {
        return Err(Error::NotBalanced(total_input, total_output));
    }

    // 4. add the new outputs
    let mut new_utxos = Vec::new();
    for (index, output) in outputs.iter().enumerate() {
        // Reject zero-valued outputs.
        if output.value == Value::zero() {
            return Err(Error::ZeroOutput(output.clone()));
        }

        if output.address.discrimination() != ledger.static_params.discrimination {
            return Err(Error::InvalidDiscrimination);
        }
        match output.address.kind() {
            Kind::Single(_) | Kind::Group(_, _) => {
                new_utxos.push((index as u8, output.clone()));
            }
            Kind::Account(identifier) => {
                // don't have a way to make a newtype ref from the ref so .clone()
                let account = identifier.clone().into();
                ledger.accounts = match ledger.accounts.add_value(&account, output.value) {
                    Ok(accounts) => accounts,
                    Err(account::LedgerError::NonExistent) if dyn_params.allow_account_creation => {
                        // if the account was not existent and that we allow creating
                        // account out of the blue, then fallback on adding the account
                        ledger.accounts.add_account(&account, output.value)?
                    }
                    Err(error) => return Err(error.into()),
                };
            }
        }
    }

    ledger.utxos = ledger.utxos.add(transaction_id, &new_utxos)?;

    Ok(ledger)
}

fn input_utxo_verify(
    mut ledger: Ledger,
    transaction_id: &TransactionId,
    utxo: &UtxoPointer,
    witness: &Witness,
) -> Result<Ledger, Error> {
    match witness {
        Witness::Account(_) => return Err(Error::ExpectingUtxoWitness),
        Witness::OldUtxo(xpub, signature) => {
            let (old_utxos, associated_output) = ledger
                .oldutxos
                .remove(&utxo.transaction_id, utxo.output_index)?;

            ledger.oldutxos = old_utxos;
            if utxo.value != associated_output.value {
                return Err(Error::UtxoValueNotMatching(
                    utxo.value,
                    associated_output.value,
                ));
            };

            if legacy::oldaddress_from_xpub(&associated_output.address, xpub) {
                return Err(Error::OldUtxoInvalidPublicKey(
                    utxo.clone(),
                    associated_output.clone(),
                    witness.clone(),
                ));
            };

            let verified = signature.verify(&xpub, &transaction_id);
            if verified == chain_crypto::Verification::Failed {
                return Err(Error::OldUtxoInvalidSignature(
                    utxo.clone(),
                    associated_output.clone(),
                    witness.clone(),
                ));
            };

            Ok(ledger)
        }
        Witness::Utxo(signature) => {
            let (new_utxos, associated_output) = ledger
                .utxos
                .remove(&utxo.transaction_id, utxo.output_index)?;
            ledger.utxos = new_utxos;
            if utxo.value != associated_output.value {
                return Err(Error::UtxoValueNotMatching(
                    utxo.value,
                    associated_output.value,
                ));
            }

            let verified = signature.verify(
                &associated_output.address.public_key().unwrap(),
                &transaction_id,
            );
            if verified == chain_crypto::Verification::Failed {
                return Err(Error::UtxoInvalidSignature(
                    utxo.clone(),
                    associated_output.clone(),
                    witness.clone(),
                ));
            };
            Ok(ledger)
        }
    }
}

fn input_account_verify(
    mut ledger: account::Ledger,
    transaction_id: &TransactionId,
    account: &account::Identifier,
    value: Value,
    witness: &Witness,
) -> Result<account::Ledger, Error> {
    // .remove_value() check if there's enough value and if not, returns a Err.
    let (new_ledger, spending_counter) = ledger.remove_value(account, value)?;
    ledger = new_ledger;

    match witness {
        Witness::OldUtxo(_, _) => return Err(Error::ExpectingAccountWitness),
        Witness::Utxo(_) => return Err(Error::ExpectingAccountWitness),
        Witness::Account(sig) => {
            let tidsc = TransactionIdSpendingCounter::new(transaction_id, &spending_counter);
            let verified = sig.verify(&account.clone().into(), &tidsc);
            if verified == chain_crypto::Verification::Failed {
                return Err(Error::AccountInvalidSignature(
                    account.clone(),
                    witness.clone(),
                ));
            };
            Ok(ledger)
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}
impl std::error::Error for Error {}

#[cfg(test)]
pub mod test {
    use super::*;
    use crate::key::{SpendingPublicKey, SpendingSecretKey};
    use chain_addr::{Address, Discrimination, Kind};
    use rand::{CryptoRng, RngCore};

    pub fn make_key<R: RngCore + CryptoRng>(
        rng: &mut R,
        discrimination: &Discrimination,
    ) -> (SpendingSecretKey, SpendingPublicKey, Address) {
        let sk = SpendingSecretKey::generate(rng);
        let pk = sk.to_public();
        let user_address = Address(discrimination.clone(), Kind::Single(pk.clone()));
        (sk, pk, user_address)
    }

    macro_rules! assert_err {
        ($left: expr, $right: expr) => {
            match &($left) {
                left_val => match &($right) {
                    Err(e) => {
                        if !(e == left_val) {
                            panic!(
                                "assertion failed: error mismatch \
                                 (left: `{:?}, right: `{:?}`)",
                                *left_val, *e
                            )
                        }
                    }
                    Ok(_) => panic!(
                        "assertion failed: expected error {:?} but got success",
                        *left_val
                    ),
                },
            }
        };
    }

    #[test]
    pub fn utxo() -> () {
        let static_params = LedgerStaticParameters {
            discrimination: Discrimination::Test,
        };
        let dyn_params = LedgerParameters {
            fees: LinearFee::new(0, 0, 0),
            allow_account_creation: true,
        };

        let mut rng = rand::thread_rng();
        let (sk1, _pk1, user1_address) = make_key(&mut rng, &static_params.discrimination);
        let (_sk2, _pk2, user2_address) = make_key(&mut rng, &static_params.discrimination);
        let tx0_id = TransactionId::hash_bytes(&[0]);
        let value = Value(42000);

        let output0 = Output {
            address: user1_address.clone(),
            value: value,
        };

        let utxo0 = UtxoPointer {
            transaction_id: tx0_id,
            output_index: 0,
            value: value,
        };
        let ledger = {
            let mut l = Ledger::new(static_params, setting::Settings::new());
            l.utxos = l.utxos.add(&tx0_id, &[(0, output0)]).unwrap();
            l
        };

        {
            let ledger = ledger.clone();
            let tx = Transaction {
                inputs: vec![Input::from_utxo(utxo0)],
                outputs: vec![Output {
                    address: user2_address.clone(),
                    value: Value(1),
                }],
                extra: NoExtra,
            };
            let signed_tx = AuthenticatedTransaction {
                transaction: tx,
                witnesses: vec![],
            };
            let r = ledger.apply_transaction(&signed_tx, &dyn_params);
            assert_err!(Error::NotEnoughSignatures(1, 0), r)
        }

        {
            let ledger = ledger.clone();
            let tx = Transaction {
                inputs: vec![Input::from_utxo(utxo0)],
                outputs: vec![Output {
                    address: user2_address.clone(),
                    value: Value(1),
                }],
                extra: NoExtra,
            };
            let txid = tx.hash();
            let w1 = Witness::new(&txid, &sk1);
            let signed_tx = AuthenticatedTransaction {
                transaction: tx,
                witnesses: vec![w1],
            };
            let r = ledger.apply_transaction(&signed_tx, &dyn_params);
            assert!(r.is_ok())
        }
    }
}