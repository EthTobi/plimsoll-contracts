extern crate std;

use super::*;
use soroban_sdk::{
    testutils::{Address as _, Events as _, Ledger as _},
    Address, BytesN, Env, String,
};

const UPGRADE_WASM: &[u8] = include_bytes!("../../../testdata/upgrade.wasm");

fn setup() -> (Env, Address, ReporterRegistryClient<'static>) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_700_000_000);
    let admin = Address::generate(&env);
    let id = env.register(ReporterRegistry, (admin.clone(),));
    let client = ReporterRegistryClient::new(&env, &id);
    (env, admin, client)
}

#[test]
fn constructor_sets_admin() {
    let (_env, admin, client) = setup();
    assert_eq!(client.admin(), admin);
}

#[test]
fn set_reporter_stores_role_and_name() {
    let (env, admin, client) = setup();
    let auditor = Address::generate(&env);
    client.set_reporter(
        &auditor,
        &Role::Auditor,
        &String::from_str(&env, "Acme Audit LLP"),
    );

    assert_eq!(env.auths()[0].0, admin);
    let info = client.get_reporter(&auditor).unwrap();
    assert_eq!(info.role, Role::Auditor);
    assert_eq!(info.name, String::from_str(&env, "Acme Audit LLP"));
    assert_eq!(info.added_at, 1_700_000_000);
    assert_eq!(client.role_of(&auditor), Some(Role::Auditor));
}

#[test]
fn set_reporter_emits_event() {
    let (env, _admin, client) = setup();
    let poster = Address::generate(&env);
    client.set_reporter(
        &poster,
        &Role::SupplyPoster,
        &String::from_str(&env, "indexer"),
    );
    assert_eq!(env.events().all().events().len(), 1);
}

#[test]
fn changing_role_keeps_added_at() {
    let (env, _admin, client) = setup();
    let who = Address::generate(&env);
    client.set_reporter(&who, &Role::Transcriber, &String::from_str(&env, "vol"));
    env.ledger().set_timestamp(1_800_000_000);
    client.set_reporter(
        &who,
        &Role::Auditor,
        &String::from_str(&env, "vol, now audit"),
    );
    let info = client.get_reporter(&who).unwrap();
    assert_eq!(info.role, Role::Auditor);
    assert_eq!(info.added_at, 1_700_000_000);
}

#[test]
fn empty_name_rejected() {
    let (env, _admin, client) = setup();
    let who = Address::generate(&env);
    let res = client.try_set_reporter(&who, &Role::Auditor, &String::from_str(&env, ""));
    assert_eq!(res, Err(Ok(RegistryError::InvalidName.into())));
}

#[test]
fn long_name_rejected() {
    let (env, _admin, client) = setup();
    let who = Address::generate(&env);
    let long = String::from_str(&env, &"x".repeat(65));
    let res = client.try_set_reporter(&who, &Role::Auditor, &long);
    assert_eq!(res, Err(Ok(RegistryError::InvalidName.into())));
}

#[test]
fn revoke_removes_reporter() {
    let (env, _admin, client) = setup();
    let who = Address::generate(&env);
    client.set_reporter(&who, &Role::Auditor, &String::from_str(&env, "a"));
    client.revoke(&who);
    assert_eq!(client.role_of(&who), None);
}

#[test]
fn revoke_unknown_fails() {
    let (env, _admin, client) = setup();
    let who = Address::generate(&env);
    let res = client.try_revoke(&who);
    assert_eq!(res, Err(Ok(RegistryError::ReporterNotFound.into())));
}

#[test]
fn unknown_reporter_has_no_role() {
    let (env, _admin, client) = setup();
    assert_eq!(client.role_of(&Address::generate(&env)), None);
}

#[test]
fn transfer_admin_requires_both() {
    let (env, admin, client) = setup();
    let next = Address::generate(&env);
    client.transfer_admin(&next);
    let auths = env.auths();
    assert!(auths.iter().any(|(a, _)| *a == admin));
    assert!(auths.iter().any(|(a, _)| *a == next));
    assert_eq!(client.admin(), next);
}

#[test]
#[should_panic]
fn set_reporter_without_auth_panics() {
    let env = Env::default();
    let admin = Address::generate(&env);
    let id = env.register(ReporterRegistry, (admin,));
    let client = ReporterRegistryClient::new(&env, &id);
    client.set_reporter(
        &Address::generate(&env),
        &Role::Auditor,
        &String::from_str(&env, "x"),
    );
}

// ---- time-locked upgrades -------------------------------------------------

fn upgrade_hash(env: &Env) -> BytesN<32> {
    env.deployer().upload_contract_wasm(UPGRADE_WASM)
}

fn pending_storage(env: &Env, id: &Address) -> Option<PendingUpgrade> {
    env.as_contract(id, || {
        env.storage().instance().get(&DataKey::PendingUpgrade)
    })
}

#[test]
fn propose_upgrade_stores_the_hash_and_execution_ledger() {
    let (env, admin, client) = setup();
    env.ledger().set_sequence_number(1_000);
    let wasm_hash = upgrade_hash(&env);
    client.propose_upgrade(&wasm_hash);

    assert_eq!(env.auths()[0].0, admin);
    assert_eq!(env.events().all().events().len(), 1);
    assert_eq!(
        client.pending_upgrade(),
        Some(PendingUpgrade {
            wasm_hash,
            executable_at_ledger: 1_000 + UPGRADE_DELAY_LEDGERS,
        })
    );
}

#[test]
fn execute_upgrade_before_the_delay_fails() {
    let (env, _admin, client) = setup();
    env.ledger().set_sequence_number(1_000);
    client.propose_upgrade(&upgrade_hash(&env));
    env.ledger()
        .set_sequence_number(1_000 + UPGRADE_DELAY_LEDGERS - 1);

    assert_eq!(
        client.try_execute_upgrade(),
        Err(Ok(RegistryError::UpgradeDelayNotElapsed.into()))
    );
    assert!(client.pending_upgrade().is_some());
}

#[test]
fn execute_upgrade_without_a_proposal_fails() {
    let (_env, _admin, client) = setup();
    assert_eq!(
        client.try_execute_upgrade(),
        Err(Ok(RegistryError::UpgradeNotProposed.into()))
    );
}

#[test]
fn execute_upgrade_after_the_delay_installs_the_wasm() {
    let (env, _admin, client) = setup();
    env.ledger().set_sequence_number(1_000);
    let wasm_hash = upgrade_hash(&env);
    client.propose_upgrade(&wasm_hash);
    env.ledger()
        .set_sequence_number(1_000 + UPGRADE_DELAY_LEDGERS);

    client.execute_upgrade();

    let events = env.events().all().filter_by_contract(&client.address);
    assert_eq!(events.events().len(), 1);
    assert_eq!(pending_storage(&env, &client.address), None);
}

#[test]
fn cancel_upgrade_clears_the_proposal() {
    let (env, _admin, client) = setup();
    env.ledger().set_sequence_number(1_000);
    client.propose_upgrade(&upgrade_hash(&env));
    client.cancel_upgrade();

    assert_eq!(env.events().all().events().len(), 1);
    assert_eq!(client.pending_upgrade(), None);
    assert_eq!(
        client.try_execute_upgrade(),
        Err(Ok(RegistryError::UpgradeNotProposed.into()))
    );
}

#[test]
fn cancel_upgrade_without_a_proposal_fails() {
    let (_env, _admin, client) = setup();
    assert_eq!(
        client.try_cancel_upgrade(),
        Err(Ok(RegistryError::UpgradeNotProposed.into()))
    );
}

#[test]
fn a_new_proposal_restarts_the_clock() {
    let (env, _admin, client) = setup();
    env.ledger().set_sequence_number(1_000);
    let first = upgrade_hash(&env);
    client.propose_upgrade(&first);

    env.ledger().set_sequence_number(1_500);
    let second = env.deployer().upload_contract_wasm(UPGRADE_WASM);
    client.propose_upgrade(&second);

    assert_eq!(
        client.pending_upgrade(),
        Some(PendingUpgrade {
            wasm_hash: second,
            executable_at_ledger: 1_500 + UPGRADE_DELAY_LEDGERS,
        })
    );
    // The first proposal's deadline no longer unlocks anything.
    env.ledger()
        .set_sequence_number(1_000 + UPGRADE_DELAY_LEDGERS);
    assert_eq!(
        client.try_execute_upgrade(),
        Err(Ok(RegistryError::UpgradeDelayNotElapsed.into()))
    );
}

#[test]
fn upgrade_steps_require_admin_auth() {
    let env = Env::default();
    let admin = Address::generate(&env);
    let id = env.register(ReporterRegistry, (admin,));
    let client = ReporterRegistryClient::new(&env, &id);
    let wasm_hash = upgrade_hash(&env);

    assert!(client.try_propose_upgrade(&wasm_hash).is_err());
    assert!(client.try_execute_upgrade().is_err());
    assert!(client.try_cancel_upgrade().is_err());
}
