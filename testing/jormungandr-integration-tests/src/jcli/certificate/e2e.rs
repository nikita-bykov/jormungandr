use crate::common::{
    jcli_wrapper::certificate::wrapper::JCLICertificateWrapper, startup::create_new_key_pair,
};

use chain_crypto::{Curve25519_2HashDH, Ed25519, SumEd25519_12};

use assert_fs::prelude::*;
use assert_fs::TempDir;
use chain_impl_mockchain::block::BlockDate;

#[test]
pub fn test_create_and_sign_new_stake_delegation() {
    let owner = create_new_key_pair::<Ed25519>();
    let kes = create_new_key_pair::<SumEd25519_12>();
    let vrf = create_new_key_pair::<Curve25519_2HashDH>();

    let certificate_wrapper = JCLICertificateWrapper::new();
    let certificate = certificate_wrapper.assert_new_stake_pool_registration(
        &kes.identifier().to_bech32_str(),
        &vrf.identifier().to_bech32_str(),
        0,
        1,
        &owner.identifier().to_bech32_str(),
        None,
    );

    let temp_dir = TempDir::new().unwrap();

    let input_file = temp_dir.child("certificate");
    input_file.write_str(&certificate).unwrap();
    let stake_pool_id = certificate_wrapper.assert_get_stake_pool_id(input_file.path());
    let certificate = certificate_wrapper
        .assert_new_stake_delegation(&stake_pool_id, &owner.identifier().to_bech32_str());

    assert_ne!(certificate, "", "delegation cert is empty");

    let signed_cert = temp_dir.child("signed_cert");
    let owner_private_key_file = temp_dir.child("owner.private");
    owner_private_key_file
        .write_str(&owner.signing_key().to_bech32_str())
        .unwrap();

    certificate_wrapper.assert_sign(
        owner_private_key_file.path(),
        input_file.path(),
        signed_cert.path(),
    );

    signed_cert.assert(crate::predicate::file_exists_and_not_empty());
}

#[test]
pub fn test_create_vote_plan_certificate() {
    let temp_dir = TempDir::new().unwrap();

    let owner = create_new_key_pair::<Ed25519>();
    let owner_private_key_file = temp_dir.child("owner.private");
    owner_private_key_file
        .write_str(&owner.signing_key().to_bech32_str())
        .unwrap();

    let certificate_wrapper = JCLICertificateWrapper::new();
    let certificate = certificate_wrapper.assert_new_vote_plan(
        "f4fdab54e2d516ce1cabe8ae8cfe77e99eeb530f7033cdf20e2392e012373a7b",
        BlockDate {
            epoch: 1,
            slot_id: 0,
        },
        BlockDate {
            epoch: 1,
            slot_id: 59,
        },
        BlockDate {
            epoch: 2,
            slot_id: 29,
        },
    );

    assert_ne!(certificate, "", "vote plan cert is empty");
}
