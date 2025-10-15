mod balances;
mod system;
mod support;
mod proof_of_existence;

use crate::{support::Dispatch, types::{AccountId, Balance}};

mod types {
    use crate::{support};

    pub type AccountId = String;
    pub type Balance = u128;
    pub type BlockNumber = u32;
    pub type Nonce = u32;   
    pub type Extrinsic = support::Extrinsic<AccountId, crate::RuntimeCall>;
    pub type Header = support::Header<BlockNumber>;
    pub type Block = support::Block<Header, Extrinsic>;
    pub type Content = &'static str;
}

pub enum RuntimeCall {
    Balances(balances::Call<Runtime>),
    ProofOfExistence(proof_of_existence::Call<Runtime>),
}

impl system::Config for Runtime {
    type AccountId = String;
    type BlockNumber = u32;
    type Nonce = u32;
}

impl balances::Config for Runtime {
    type Balance = u128;
}

impl proof_of_existence::Config for Runtime {
    type Content = types::Content;
}

#[derive(Debug)]
pub struct Runtime {
    balances: balances::Pallet<Runtime>,
    system: system::Pallet<Runtime>,
    proof_of_existence: proof_of_existence::Pallet<Runtime>,
}

impl crate::support::Dispatch for Runtime {
    type Caller = <Runtime as system::Config>::AccountId;
    type Call = RuntimeCall;
    
    fn dispatch(
        &mut self,
        caller: Self::Caller,
        runtime_call: Self::Call) -> support::DispatchResult
    {
        match runtime_call {
            RuntimeCall::Balances(call) => {
                self.balances.dispatch(caller, call)?;
            },
            RuntimeCall::ProofOfExistence(call) => {
                self.proof_of_existence.dispatch(caller, call)?;
            },
        }

        Ok(())
    }
}

impl Runtime {
    pub fn new() -> Self {
        Self {
            balances: balances::Pallet::new(),
            system: system::Pallet::new(),
            proof_of_existence: proof_of_existence::Pallet::new(),
        }
    }

    fn execute_block(&mut self, block: types::Block) -> support::DispatchResult {
        self.system.inc_block();

        if self.system.get_block_number() != block.header.block_number {
            return Err("block number mismatch");
        } 

        for (i, support::Extrinsic {caller, call}) in block.extrinsics.into_iter().enumerate() {
            self.system.inc_nonce(&caller);
            let _ = self.dispatch(caller, call).map_err(|e| {
                eprintln!("Extrinsic Error\n\tBlockNumber: {}\n\tExtrinsic number: {}\n\tError: {}",
                block.header.block_number, i, e);
            });            
        }

        Ok(())
    }   
}

fn main() {
    let mut runner = Runtime::new();
    let alice = "Alice".to_string();
    let bob = "Bob".to_string();
    let charlie = "Charlie".to_string();
 
    runner.balances.set_balance(&alice, 100);

    let block1 = types::Block {
        header: types::Header { block_number: 1 },
        extrinsics: vec![
            types::Extrinsic {
                caller: alice.clone(),
                call: RuntimeCall::Balances(balances::Call::Transfer { to: bob.clone(), amount: 30 }),
            },
            types::Extrinsic {
                caller: bob.clone(),
                call: RuntimeCall::Balances(balances::Call::Transfer { to: charlie.clone(), amount: 20 }),
            },
        ],
    };

    let _ = runner.execute_block(block1);

    let block2 = types::Block {
        header: types::Header { block_number: 2 },
        extrinsics: vec![
            types::Extrinsic {
                caller: alice.clone(),
                call: RuntimeCall::ProofOfExistence(proof_of_existence::Call::CreateClaim { claim: "Alice documents!" }),
            },
            types::Extrinsic {
                caller: bob.clone(),
                call: RuntimeCall::ProofOfExistence(proof_of_existence::Call::CreateClaim { claim: "Bob documents!" }),
            },
        ],
    };

    let _ = runner.execute_block(block2);

    println!("{:?}", runner);
}