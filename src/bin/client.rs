//! Localnet helper: initializes the beandx config (payer = super admin and
//! treasury) if it doesn't exist yet, then prints it.

#[cfg(not(target_os = "solana"))]
fn main() {
    use beandx::{
        constants::RECEIPT_DECIMALS,
        instruction::{find_config_address, initialize_config},
        state::Config,
    };
    use borsh::BorshDeserialize;
    use solana_client::rpc_client::RpcClient;
    use solana_keypair::read_keypair_file;
    use solana_signer::Signer;
    use solana_transaction::Transaction;

    const WITHDRAW_FEE_BPS: u16 = 20;

    let rpc = RpcClient::new("http://127.0.0.1:8899".to_string());
    let keypair_path = std::env::var("KEYPAIR")
        .unwrap_or_else(|_| format!("{}/.config/solana/id.json", std::env::var("HOME").unwrap()));
    let payer = read_keypair_file(&keypair_path).expect("failed to read keypair");
    println!("payer {}", payer.pubkey());

    let config_address = find_config_address().0;
    if rpc.get_account(&config_address).is_err() {
        let ix = initialize_config(&payer.pubkey(), &payer.pubkey(), WITHDRAW_FEE_BPS);
        let blockhash = rpc.get_latest_blockhash().expect("failed to get blockhash");
        let tx = Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer], blockhash);
        let signature = rpc.send_and_confirm_transaction(&tx).expect("initialize_config failed");
        println!("initialized config: {signature}");
    }

    let data = rpc.get_account_data(&config_address).expect("config missing");
    let config = Config::deserialize(&mut &data[..]).expect("invalid config");
    println!("config {config_address}");
    println!("  super admin   {}", config.super_admin);
    println!("  treasury      {}", config.treasury);
    println!("  withdraw fee  {} bps", config.withdraw_fee_bps);
    println!("  admins        {:?}", config.admins);
    println!("  share decimals {RECEIPT_DECIMALS}");
}

#[cfg(target_os = "solana")]
fn main() {}
