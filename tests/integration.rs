#![allow(clippy::result_large_err)]
//! End-to-end tests on LiteSVM. Requires `cargo build-sbf` first.
//!
//! The NVDA/TSLA/GOOGL mints are real Backpack Securities mint accounts dumped
//! from mainnet (`tests/fixtures`), with the mint authority patched to a test
//! key, so the Token-2022 extensions (pausable, permanent delegate, scaled UI
//! amount, unset transfer hook, ...) are exactly what the program sees in prod.

use beandx::{
    constants::{SYSTEM_PROGRAM_ID, TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID},
    error::BeandxError,
    instruction::{self as ix, AssetKeys},
    math,
    state::{Config, Pool},
    token,
};
use borsh::BorshDeserialize;
use litesvm::{
    LiteSVM,
    types::{FailedTransactionMetadata, TransactionMetadata},
};
use solana_account::Account;
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_pubkey::{Pubkey, pubkey};
use solana_signer::Signer;
use solana_transaction::Transaction;

const NVDA: Pubkey = pubkey!("NVDAVuiB7hwd3m5Wa1JuHNovPaPG6BH1QNztbKFxNjv");
const TSLA: Pubkey = pubkey!("TSLAqBbv4CNCnzWFeB7LmydAyEiNMJtve7DYKLpdK4S");
const GOOGL: Pubkey = pubkey!("GooGLMvZtJZXHdNZy95RUrAMCCKxV6ghK3qE5ddBgVm5");
const COMPUTE_BUDGET_ID: Pubkey = pubkey!("ComputeBudget111111111111111111111111111111");
const SYMBOL: &str = "AIX";
const FEE_BPS: u16 = 20;

struct Env {
    svm: LiteSVM,
    super_admin: Keypair,
    admin: Keypair,
    treasury: Keypair,
    issuer: Keypair,
    assets: Vec<AssetKeys>,
}

impl Env {
    /// Program loaded, config initialized, `admin` added, assets mintable.
    fn new() -> Self {
        let mut svm = LiteSVM::new();
        let so = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/target/deploy/beandx.so"))
            .expect("run `cargo build-sbf` before the integration tests");
        svm.add_program(beandx::ID, &so).unwrap();

        let super_admin = Keypair::new();
        let admin = Keypair::new();
        let treasury = Keypair::new();
        let issuer = Keypair::new();
        for kp in [&super_admin, &admin, &treasury, &issuer] {
            svm.airdrop(&kp.pubkey(), 100_000_000_000).unwrap();
        }
        set_upgrade_authority(&mut svm, &super_admin.pubkey());

        for (address, bytes) in [
            (NVDA, &include_bytes!("fixtures/backpack_nvda_mint.bin")[..]),
            (TSLA, &include_bytes!("fixtures/backpack_tsla_mint.bin")[..]),
            (GOOGL, &include_bytes!("fixtures/backpack_googl_mint.bin")[..]),
        ] {
            load_mint_fixture(&mut svm, address, bytes, &issuer.pubkey());
        }
        // A classic SPL Token asset with different decimals, for coverage.
        let classic = create_classic_mint(&mut svm, &issuer, 9);

        let assets = [NVDA, TSLA, GOOGL]
            .into_iter()
            .map(|mint| AssetKeys { mint, token_program: TOKEN_2022_PROGRAM_ID })
            .chain([AssetKeys { mint: classic, token_program: TOKEN_PROGRAM_ID }])
            .collect();

        let mut env = Self { svm, super_admin, admin, treasury, issuer, assets };
        let (sa, admin_key) = (env.super_admin.insecure_clone(), env.admin.pubkey());
        env.send(&[ix::initialize_config(&sa.pubkey(), &env.treasury.pubkey(), FEE_BPS)], &sa)
            .unwrap();
        env.send(&[ix::add_admin(&sa.pubkey(), &admin_key)], &sa).unwrap();
        env
    }

    /// `new()` plus an AIX pool seeded by the admin.
    fn with_seeded_pool() -> Self {
        let mut env = Self::new();
        env.create_pool(SYMBOL).unwrap();
        let admin = env.admin.insecure_clone();
        env.fund(&admin.pubkey(), 100_000_000_000);
        env.send(
            &[ix::seed_pool(&admin.pubkey(), SYMBOL, &env.assets, 1_000_000_000, seed_amounts())],
            &admin,
        )
        .unwrap();
        env
    }

    fn create_pool(&mut self, symbol: &str) -> Result<TransactionMetadata, FailedTransactionMetadata> {
        let admin = self.admin.insecure_clone();
        self.send(
            &[
                compute_limit(1_400_000),
                ix::create_pool(
                    &admin.pubkey(),
                    symbol,
                    "AI Basket",
                    "https://beandx.xyz/aix.json",
                    &self.assets,
                    vec![4_000, 3_000, 2_000, 1_000],
                ),
            ],
            &admin,
        )
    }

    fn send(&mut self, ixs: &[Instruction], signer: &Keypair) -> Result<TransactionMetadata, FailedTransactionMetadata> {
        let tx = Transaction::new_signed_with_payer(ixs, Some(&signer.pubkey()), &[signer], self.svm.latest_blockhash());
        let result = self.svm.send_transaction(tx);
        self.svm.expire_blockhash();
        result
    }

    fn new_user(&mut self) -> Keypair {
        let user = Keypair::new();
        self.svm.airdrop(&user.pubkey(), 10_000_000_000).unwrap();
        user
    }

    /// Creates `owner`'s ATAs for every asset (and the receipt token, if the
    /// pool exists) and mints `amount` raw units of each asset to them.
    fn fund(&mut self, owner: &Pubkey, amount: u64) {
        let issuer = self.issuer.insecure_clone();
        let mut ixs = vec![];
        for asset in &self.assets {
            ixs.push(token::create_associated_token_account_idempotent(
                &issuer.pubkey(),
                owner,
                &asset.mint,
                &asset.token_program,
            ));
            ixs.push(token::mint_to_checked(
                &asset.token_program,
                &asset.mint,
                &token::get_associated_token_address(owner, &asset.mint, &asset.token_program),
                &issuer.pubkey(),
                amount,
                self.decimals(&asset.mint),
            ));
        }
        self.send(&ixs, &issuer).unwrap();
        self.create_receipt_ata(owner);
    }

    fn create_receipt_ata(&mut self, owner: &Pubkey) {
        let receipt_mint = self.receipt_mint();
        if self.svm.get_account(&receipt_mint).is_none() {
            return;
        }
        let issuer = self.issuer.insecure_clone();
        self.send(
            &[token::create_associated_token_account_idempotent(
                &issuer.pubkey(),
                owner,
                &receipt_mint,
                &TOKEN_2022_PROGRAM_ID,
            )],
            &issuer,
        )
        .unwrap();
    }

    fn decimals(&self, mint: &Pubkey) -> u8 {
        self.svm.get_account(mint).unwrap().data[44]
    }

    fn receipt_mint(&self) -> Pubkey {
        ix::find_receipt_mint_address(&ix::find_pool_address(SYMBOL).0).0
    }

    fn supply(&self) -> u64 {
        let data = self.svm.get_account(&self.receipt_mint()).unwrap().data;
        u64::from_le_bytes(data[36..44].try_into().unwrap())
    }

    fn balance(&self, token_account: &Pubkey) -> u64 {
        let data = self.svm.get_account(token_account).unwrap().data;
        u64::from_le_bytes(data[64..72].try_into().unwrap())
    }

    fn shares_of(&self, owner: &Pubkey) -> u64 {
        self.balance(&token::get_associated_token_address(owner, &self.receipt_mint(), &TOKEN_2022_PROGRAM_ID))
    }

    fn asset_balances(&self, owner: &Pubkey) -> Vec<u64> {
        self.assets
            .iter()
            .map(|a| self.balance(&token::get_associated_token_address(owner, &a.mint, &a.token_program)))
            .collect()
    }

    fn vault_balances(&self) -> Vec<u64> {
        self.asset_balances(&ix::find_pool_address(SYMBOL).0)
    }

    fn pool(&self) -> Pool {
        let data = self.svm.get_account(&ix::find_pool_address(SYMBOL).0).unwrap().data;
        Pool::deserialize(&mut &data[..]).unwrap()
    }

    fn config(&self) -> Config {
        let data = self.svm.get_account(&ix::find_config_address().0).unwrap().data;
        Config::deserialize(&mut &data[..]).unwrap()
    }
}

/// ~2 NVDA, 1 TSLA, 1.5 GOOGL, 5 TEST for 1,000 shares.
fn seed_amounts() -> Vec<u64> {
    vec![2_000_000, 1_000_000, 1_500_000, 5_000_000_000]
}

fn set_upgrade_authority(svm: &mut LiteSVM, authority: &Pubkey) {
    let address = ix::find_program_data_address();
    let mut account = svm.get_account(&address).unwrap();
    account.data[12] = 1;
    account.data[13..45].copy_from_slice(authority.as_ref());
    svm.set_account(address, account).unwrap();
}

fn load_mint_fixture(svm: &mut LiteSVM, address: Pubkey, bytes: &[u8], mint_authority: &Pubkey) {
    let mut data = bytes.to_vec();
    data[0..4].copy_from_slice(&1u32.to_le_bytes());
    data[4..36].copy_from_slice(mint_authority.as_ref());
    let account = Account {
        lamports: svm.minimum_balance_for_rent_exemption(data.len()),
        data,
        owner: TOKEN_2022_PROGRAM_ID,
        executable: false,
        rent_epoch: 0,
    };
    svm.set_account(address, account).unwrap();
}

fn create_classic_mint(svm: &mut LiteSVM, authority: &Keypair, decimals: u8) -> Pubkey {
    let mint = Keypair::new();
    let ixs = [
        token::create_account(
            &authority.pubkey(),
            &mint.pubkey(),
            svm.minimum_balance_for_rent_exemption(82),
            82,
            &TOKEN_PROGRAM_ID,
        ),
        token::initialize_mint2(&TOKEN_PROGRAM_ID, &mint.pubkey(), &authority.pubkey(), decimals),
    ];
    let tx = Transaction::new_signed_with_payer(&ixs, Some(&authority.pubkey()), &[authority, &mint], svm.latest_blockhash());
    svm.send_transaction(tx).unwrap();
    mint.pubkey()
}

fn compute_limit(units: u32) -> Instruction {
    let mut data = vec![2];
    data.extend_from_slice(&units.to_le_bytes());
    Instruction { program_id: COMPUTE_BUDGET_ID, accounts: vec![], data }
}

fn assert_err(result: Result<TransactionMetadata, FailedTransactionMetadata>, expected: BeandxError) {
    let err = result.expect_err("transaction should have failed");
    let code = format!("Custom({})", expected as u32);
    assert!(
        format!("{:?}", err.err).contains(&code),
        "expected {expected:?} ({code}), got {:?}\n{}",
        err.err,
        err.meta.logs.join("\n")
    );
}

/// Every share must stay backed by at least as much of every asset as before.
fn assert_backing_not_diluted(before: &[u64], supply_before: u64, after: &[u64], supply_after: u64) {
    for (b, a) in before.iter().zip(after) {
        assert!(*a as u128 * supply_before as u128 >= *b as u128 * supply_after as u128);
    }
}

#[test]
fn create_pool_sets_up_mint_vaults_and_metadata() {
    let mut env = Env::new();
    let result = env.create_pool(SYMBOL).unwrap();
    println!("create_pool (4 assets): {} CU", result.compute_units_consumed);

    let pool_address = ix::find_pool_address(SYMBOL).0;
    let pool = env.pool();
    assert_eq!(pool.symbol, SYMBOL);
    assert!(!pool.paused);
    assert_eq!(pool.assets.len(), 4);
    assert_eq!(pool.assets.iter().map(|a| a.weight_bps).collect::<Vec<_>>(), [4_000, 3_000, 2_000, 1_000]);
    for (stored, keys) in pool.assets.iter().zip(&env.assets) {
        assert_eq!(stored.mint, keys.mint);
        assert_eq!(stored.vault, token::get_associated_token_address(&pool_address, &keys.mint, &keys.token_program));
        let vault = env.svm.get_account(&stored.vault).expect("vault exists");
        assert_eq!(vault.owner, keys.token_program);
        assert_eq!(&vault.data[32..64], pool_address.as_ref());
    }
    assert_eq!(pool.assets[0].decimals, 6);
    assert_eq!(pool.assets[3].decimals, 9);

    let mint = env.svm.get_account(&pool.mint).unwrap();
    assert_eq!(mint.owner, TOKEN_2022_PROGRAM_ID);
    assert_eq!(&mint.data[4..36], pool_address.as_ref(), "pool is mint authority");
    assert_eq!(mint.data[44], 6);
    assert_eq!(mint.data[46], 0, "no freeze authority");
    assert_eq!(mint.data.len(), token::receipt_mint_len("AI Basket", SYMBOL, "https://beandx.xyz/aix.json"));
    let contains = |needle: &[u8]| mint.data.windows(needle.len()).any(|w| w == needle);
    assert!(contains(b"AI Basket") && contains(b"AIX") && contains(b"https://beandx.xyz/aix.json"));

    // Same symbol again is rejected.
    assert_err(env.create_pool(SYMBOL), BeandxError::AlreadyInitialized);
}

#[test]
fn seed_deposit_withdraw_lifecycle() {
    let mut env = Env::with_seeded_pool();
    let admin = env.admin.pubkey();
    assert_eq!(env.shares_of(&admin), 1_000_000_000);
    assert_eq!(env.vault_balances(), seed_amounts());

    // Seeding is one-time while shares exist.
    let admin_kp = env.admin.insecure_clone();
    assert_err(
        env.send(&[ix::seed_pool(&admin, SYMBOL, &env.assets, 1, vec![1; 4])], &admin_kp),
        BeandxError::PoolAlreadySeeded,
    );

    // --- deposit ---
    let user = env.new_user();
    env.fund(&user.pubkey(), 50_000_000_000);
    let shares = 123_456_789;
    let supply = env.supply();
    let vaults = env.vault_balances();
    let user_before = env.asset_balances(&user.pubkey());
    let required: Vec<u64> = vaults.iter().map(|v| math::deposit_amount(*v, shares, supply).unwrap()).collect();

    // One unit short on any asset trips the slippage guard.
    let mut too_tight = required.clone();
    too_tight[2] -= 1;
    assert_err(
        env.send(&[ix::deposit(&user.pubkey(), SYMBOL, &env.assets, shares, too_tight)], &user),
        BeandxError::SlippageExceeded,
    );

    let result = env.send(&[ix::deposit(&user.pubkey(), SYMBOL, &env.assets, shares, required.clone())], &user).unwrap();
    println!("deposit (4 assets): {} CU", result.compute_units_consumed);
    assert_eq!(env.shares_of(&user.pubkey()), shares);
    let user_after = env.asset_balances(&user.pubkey());
    for i in 0..4 {
        assert_eq!(user_before[i] - user_after[i], required[i]);
    }
    assert_backing_not_diluted(&vaults, supply, &env.vault_balances(), env.supply());

    // max_mintable_shares gives a share amount the wallet can always afford.
    let max = math::max_mintable_shares(&env.asset_balances(&user.pubkey()), &env.vault_balances(), env.supply());
    assert!(max > 0);

    // --- withdraw ---
    env.create_receipt_ata(&env.treasury.pubkey());
    let shares_out = 60_000_000;
    let fee = math::withdraw_fee(shares_out, FEE_BPS).unwrap();
    let burn = shares_out - fee;
    let supply = env.supply();
    let vaults = env.vault_balances();
    let expected: Vec<u64> = vaults.iter().map(|v| math::withdraw_amount(*v, burn, supply).unwrap()).collect();
    let user_before = env.asset_balances(&user.pubkey());

    let mut too_greedy = expected.clone();
    too_greedy[0] += 1;
    let treasury = env.treasury.pubkey();
    assert_err(
        env.send(&[ix::withdraw(&user.pubkey(), &treasury, SYMBOL, &env.assets, shares_out, too_greedy)], &user),
        BeandxError::SlippageExceeded,
    );

    let result = env
        .send(&[ix::withdraw(&user.pubkey(), &treasury, SYMBOL, &env.assets, shares_out, expected.clone())], &user)
        .unwrap();
    println!("withdraw (4 assets): {} CU", result.compute_units_consumed);
    assert_eq!(fee, 120_000, "20 bps of 60 shares");
    assert_eq!(env.shares_of(&treasury), fee);
    assert_eq!(env.shares_of(&user.pubkey()), shares - shares_out);
    assert_eq!(env.supply(), supply - burn);
    let user_after = env.asset_balances(&user.pubkey());
    for i in 0..4 {
        assert_eq!(user_after[i] - user_before[i], expected[i]);
    }
    assert_backing_not_diluted(&vaults, supply, &env.vault_balances(), env.supply());

    // Fees can't be dodged by pointing at a different receipt account.
    let user_kp = user.insecure_clone();
    assert_err(
        env.send(&[ix::withdraw(&user.pubkey(), &user.pubkey(), SYMBOL, &env.assets, 1_000_000, vec![0; 4])], &user_kp),
        BeandxError::InvalidTreasuryAccount,
    );
}

#[test]
fn deposit_requires_seed() {
    let mut env = Env::new();
    env.create_pool(SYMBOL).unwrap();
    let user = env.new_user();
    env.fund(&user.pubkey(), 1_000_000_000);
    assert_err(
        env.send(&[ix::deposit(&user.pubkey(), SYMBOL, &env.assets, 1_000_000, vec![u64::MAX; 4])], &user),
        BeandxError::PoolNotSeeded,
    );
}

#[test]
fn pause_blocks_deposits_but_not_withdrawals() {
    let mut env = Env::with_seeded_pool();
    let admin = env.admin.insecure_clone();
    let user = env.new_user();
    env.fund(&user.pubkey(), 50_000_000_000);
    env.send(&[ix::deposit(&user.pubkey(), SYMBOL, &env.assets, 10_000_000, vec![u64::MAX; 4])], &user)
        .unwrap();

    env.send(&[ix::set_paused(&admin.pubkey(), SYMBOL, true)], &admin).unwrap();
    assert!(env.pool().paused);
    assert_err(
        env.send(&[ix::deposit(&user.pubkey(), SYMBOL, &env.assets, 1_000_000, vec![u64::MAX; 4])], &user),
        BeandxError::PoolPaused,
    );
    env.create_receipt_ata(&env.treasury.pubkey());
    let treasury = env.treasury.pubkey();
    env.send(&[ix::withdraw(&user.pubkey(), &treasury, SYMBOL, &env.assets, 10_000_000, vec![0; 4])], &user)
        .unwrap();
    assert_eq!(env.shares_of(&user.pubkey()), 0);

    env.send(&[ix::set_paused(&admin.pubkey(), SYMBOL, false)], &admin).unwrap();
    env.send(&[ix::deposit(&user.pubkey(), SYMBOL, &env.assets, 1_000_000, vec![u64::MAX; 4])], &user)
        .unwrap();
}

#[test]
fn admin_rebalance_and_weights() {
    let mut env = Env::with_seeded_pool();
    let admin = env.admin.insecure_clone();
    let vaults = env.vault_balances();

    // Sell 0.5 NVDA out of the pool for 0.8 TSLA.
    let (nvda, tsla) = (env.assets[0], env.assets[1]);
    env.send(&[ix::rebalance(&admin.pubkey(), SYMBOL, 0, &nvda, 500_000, 1, &tsla, 800_000)], &admin)
        .unwrap();
    let after = env.vault_balances();
    assert_eq!(after[0], vaults[0] - 500_000);
    assert_eq!(after[1], vaults[1] + 800_000);
    assert_eq!(after[2..], vaults[2..]);

    assert_err(
        env.send(&[ix::rebalance(&admin.pubkey(), SYMBOL, 0, &nvda, 1, 0, &nvda, 1)], &admin),
        BeandxError::InvalidAssetIndex,
    );
    // Accounts for a different asset than the index claims.
    assert_err(
        env.send(&[ix::rebalance(&admin.pubkey(), SYMBOL, 0, &tsla, 1, 1, &nvda, 1)], &admin),
        BeandxError::InvalidAssetAccount,
    );

    env.send(&[ix::set_weights(&admin.pubkey(), SYMBOL, vec![2_500, 2_500, 2_500, 2_500])], &admin)
        .unwrap();
    assert!(env.pool().assets.iter().all(|a| a.weight_bps == 2_500));
    assert_err(
        env.send(&[ix::set_weights(&admin.pubkey(), SYMBOL, vec![5_000, 5_000, 1, 0])], &admin),
        BeandxError::InvalidWeights,
    );
    assert_err(
        env.send(&[ix::set_weights(&admin.pubkey(), SYMBOL, vec![10_000])], &admin),
        BeandxError::InvalidWeights,
    );
}

#[test]
fn access_control() {
    let mut env = Env::with_seeded_pool();
    let outsider = env.new_user();
    env.fund(&outsider.pubkey(), 1_000_000_000);
    let (nvda, tsla) = (env.assets[0], env.assets[1]);

    // Only the upgrade authority could have initialized, and only once.
    let sa = env.super_admin.insecure_clone();
    assert!(env.send(&[ix::initialize_config(&sa.pubkey(), &sa.pubkey(), 0)], &sa).is_err());
    assert_eq!(env.config().super_admin, sa.pubkey());

    for instruction in [
        ix::add_admin(&outsider.pubkey(), &outsider.pubkey()),
        ix::update_config(&outsider.pubkey(), Some(outsider.pubkey()), None, None),
        ix::set_paused(&outsider.pubkey(), SYMBOL, true),
        ix::set_weights(&outsider.pubkey(), SYMBOL, vec![2_500; 4]),
        ix::rebalance(&outsider.pubkey(), SYMBOL, 0, &nvda, 1, 1, &tsla, 1),
    ] {
        assert_err(env.send(&[instruction], &outsider), BeandxError::Unauthorized);
    }

    // Removed admins lose access; the super admin is always an admin.
    let admin = env.admin.pubkey();
    env.send(&[ix::remove_admin(&sa.pubkey(), &admin)], &sa).unwrap();
    assert!(env.config().admins.is_empty());
    assert_err(env.create_pool("AIX2"), BeandxError::Unauthorized);
    env.send(&[ix::set_paused(&sa.pubkey(), SYMBOL, true)], &sa).unwrap();

    assert_err(
        env.send(&[ix::update_config(&sa.pubkey(), None, None, Some(501))], &sa),
        BeandxError::FeeTooHigh,
    );
    env.send(&[ix::update_config(&sa.pubkey(), None, None, Some(30))], &sa).unwrap();
    assert_eq!(env.config().withdraw_fee_bps, 30);
}

#[test]
fn initialize_config_requires_upgrade_authority() {
    let mut svm = LiteSVM::new();
    let so = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/target/deploy/beandx.so")).unwrap();
    svm.add_program(beandx::ID, &so).unwrap();
    let real = Keypair::new();
    let attacker = Keypair::new();
    svm.airdrop(&attacker.pubkey(), 1_000_000_000).unwrap();
    set_upgrade_authority(&mut svm, &real.pubkey());

    let tx = Transaction::new_signed_with_payer(
        &[ix::initialize_config(&attacker.pubkey(), &attacker.pubkey(), 20)],
        Some(&attacker.pubkey()),
        &[&attacker],
        svm.latest_blockhash(),
    );
    assert_err(svm.send_transaction(tx), BeandxError::Unauthorized);
}

#[test]
fn create_pool_validation() {
    let mut env = Env::new();
    let admin = env.admin.insecure_clone();
    let assets = env.assets.clone();
    let try_create = |env: &mut Env, symbol: &str, assets: &[AssetKeys], weights: Vec<u16>| {
        env.send(
            &[compute_limit(1_400_000), ix::create_pool(&admin.pubkey(), symbol, "Basket", "", assets, weights)],
            &admin,
        )
    };

    assert_err(try_create(&mut env, "AI-X", &assets, vec![4_000, 3_000, 2_000, 1_000]), BeandxError::InvalidSymbol);
    assert_err(try_create(&mut env, "WAYTOOLONGSYM", &assets, vec![4_000, 3_000, 2_000, 1_000]), BeandxError::InvalidSymbol);
    assert_err(try_create(&mut env, "AIX", &assets, vec![4_000, 3_000, 2_000, 999]), BeandxError::InvalidWeights);
    assert_err(try_create(&mut env, "AIX", &assets, vec![4_000, 3_000, 3_000, 0]), BeandxError::InvalidWeights);
    assert_err(try_create(&mut env, "AIX", &[assets[0], assets[0]], vec![5_000, 5_000]), BeandxError::DuplicateAsset);
    assert_err(try_create(&mut env, "AIX", &[], vec![]), BeandxError::InvalidAssetCount);

    // Asset claimed under the wrong token program.
    let wrong_program = AssetKeys { mint: NVDA, token_program: TOKEN_PROGRAM_ID };
    assert_err(try_create(&mut env, "AIX", &[wrong_program], vec![10_000]), BeandxError::InvalidTokenProgram);

    // If an issuer turns on a transfer hook, the mint is rejected: our plain
    // TransferChecked CPIs don't forward hook accounts.
    let hooked = Pubkey::new_unique();
    let mut data = include_bytes!("fixtures/backpack_nvda_mint.bin").to_vec();
    let hook_program = transfer_hook_program_offset(&data);
    data[hook_program..hook_program + 32].copy_from_slice(Pubkey::new_unique().as_ref());
    load_mint_fixture(&mut env.svm, hooked, &data, &env.issuer.pubkey());
    let hooked = AssetKeys { mint: hooked, token_program: TOKEN_2022_PROGRAM_ID };
    assert_err(try_create(&mut env, "HOOK", &[hooked], vec![10_000]), BeandxError::UnsupportedMint);

    // Non-mint account passed as an asset.
    let not_a_mint = AssetKeys { mint: SYSTEM_PROGRAM_ID, token_program: TOKEN_PROGRAM_ID };
    assert_err(try_create(&mut env, "AIX", &[not_a_mint], vec![10_000]), BeandxError::InvalidTokenProgram);

    // Nothing above left a pool behind.
    assert!(env.svm.get_account(&ix::find_pool_address("AIX").0).is_none());
    try_create(&mut env, "AIX", &assets, vec![4_000, 3_000, 2_000, 1_000]).unwrap();
}

/// Offset of TransferHook.program_id inside a Token-2022 mint's TLV data.
fn transfer_hook_program_offset(data: &[u8]) -> usize {
    let mut offset = 166;
    loop {
        let ext = u16::from_le_bytes([data[offset], data[offset + 1]]);
        let len = u16::from_le_bytes([data[offset + 2], data[offset + 3]]) as usize;
        if ext == 14 {
            return offset + 4 + 32;
        }
        offset += 4 + len;
    }
}
