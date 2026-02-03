use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    system_program,
    transaction::Transaction,
};
use std::str::FromStr;
use counter_program::CounterInstruction;

#[tokio::main]
async fn main() {
    // Replace with your actual program ID from deployment
    let program_id = Pubkey::from_str("6HPHrR4me8sLFZGZxebXz856RF5DwZVforHX7sySPfPL")
        .expect("Invalid program ID");

    // Connect to local cluster
    let rpc_url = String::from("http://localhost:8899");
    let client = RpcClient::new_with_commitment(rpc_url, CommitmentConfig::confirmed());

    // Generate a new keypair for paying fees
    let payer = Keypair::new();

    // Request airdrop of 1 SOL for transaction fees
    println!("Requesting airdrop...");
    let airdrop_signature = client
        .request_airdrop(&payer.pubkey(), 1_000_000_000)
        .expect("Failed to request airdrop");

    // Wait for airdrop confirmation
    loop {
        if client
            .confirm_transaction(&airdrop_signature)
            .unwrap_or(false)
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    println!("Airdrop confirmed");

    println!("\nInitializing counter...");
    let counter_keypair = Keypair::new();
    let initial_value = 100u64;

    // Serialize the initialize instruction data
    let instruction_data = borsh::to_vec(&CounterInstruction::Initialize { initial_value })
        .expect("Failed to serialize instruction");

    let initialize_instruction = Instruction::new_with_bytes(
        program_id,
        &instruction_data,
        vec![
            AccountMeta::new(counter_keypair.pubkey(), true),
            AccountMeta::new(payer.pubkey(), true),
            AccountMeta::new_readonly(system_program::id(), false),
        ],
    );

    let mut transaction =
        Transaction::new_with_payer(&[initialize_instruction], Some(&payer.pubkey()));

    let blockhash = client
        .get_latest_blockhash()
        .expect("Failed to get blockhash");
    transaction.sign(&[&payer, &counter_keypair], blockhash);

    match client.send_and_confirm_transaction(&transaction) {
        Ok(signature) => {
            println!("Counter initialized!");
            println!("Transaction: {}", signature);
            println!("Counter address: {}", counter_keypair.pubkey());
        }
        Err(err) => {
            eprintln!("Failed to initialize counter: {}", err);
            return;
        }
    }

    println!("\nIncrementing counter...");
    // Serialize the increment instruction data
    let increment_data = borsh::to_vec(&CounterInstruction::Increment)
        .expect("Failed to serialize instruction");

    let increment_instruction = Instruction::new_with_bytes(
        program_id,
        &increment_data,
        vec![AccountMeta::new(counter_keypair.pubkey(), true)],
    );

    let mut transaction =
        Transaction::new_with_payer(&[increment_instruction], Some(&payer.pubkey()));

    transaction.sign(&[&payer, &counter_keypair], blockhash);

    match client.send_and_confirm_transaction(&transaction) {
        Ok(signature) => {
            println!("Counter incremented!");
            println!("Transaction: {}", signature);
        }
        Err(err) => {
            eprintln!("Failed to increment counter: {}", err);
        }
    }
}