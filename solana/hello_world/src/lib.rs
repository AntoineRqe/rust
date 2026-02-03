use solana_program::{
    account_info::AccountInfo, entrypoint, entrypoint::ProgramResult, msg, pubkey::Pubkey,
};

entrypoint!(process_instruction);

pub fn process_instruction(
    _program_id: &Pubkey,
    _accounts: &[AccountInfo],
    _instruction_data: &[u8],
) -> ProgramResult {
    msg!("Hello, solana!");
    Ok(())
}

#[cfg(test)]
mod test {
    use litesvm::LiteSVM;
    use solana_sdk::{
        instruction::Instruction,
        message::Message,
        signature::{Keypair, Signer},
        transaction::Transaction,
    };

    #[test]
    fn test_hello_world() {
        // Create a new LiteSVM instance
        let mut svm = LiteSVM::new();

        // Create a keypair for the transaction payer
        let payer = Keypair::new();

        // Airdrop some lamports to the payer
        svm.airdrop(&payer.pubkey(), 1_000_000_000).unwrap();

        // Load our program
        let program_keypair = Keypair::new();
        let program_id = program_keypair.pubkey();
        svm.add_program_from_file(program_id, "target/deploy/hello_world.so")
            .unwrap();

        // Create instruction with no accounts and no data
        let instruction = Instruction {
            program_id,
            accounts: vec![],
            data: vec![],
        };

        // Create transaction
        let message = Message::new(&[instruction], Some(&payer.pubkey()));
        let transaction = Transaction::new(&[&payer], message, svm.latest_blockhash());

        // Send transaction and verify it succeeds
        let result = svm.send_transaction(transaction);
        assert!(result.is_ok(), "Transaction should succeed");
        let logs = result.unwrap().logs;
        println!("Logs: {logs:#?}");
    }
}