mod balances;
mod system;

mod types {
    pub type AccountId = String;
    pub type Balance = u128;
    pub type BlockNumber = u32;
    pub type Nonce = u32;
}

#[derive(Debug)]
pub struct Runtime {
    balances: balances::Pallet<types::AccountId, types::Balance>,
    system: system::Pallet<types::AccountId, types::BlockNumber, types::Nonce>,
}

impl Runtime {
    pub fn new() -> Self {
        Self {
            balances: balances::Pallet::new(),
            system: system::Pallet::new(),
        }
    }
}

fn main() {
    let mut runner = Runtime::new();
    let alice = "Alice".to_string();
    let bob = "Bob".to_string();
    let charlie = "Charlie".to_string();
 
    runner.balances.set_balance(&alice, 100);

    runner.system.inc_block();
    runner.system.inc_nonce(&alice);

    assert!(runner.system.get_block_number() == 1);
    assert!(runner.system.get_nonce(&alice) == 1);

    let _ = runner.balances
        .transfer(&alice, &bob, 30)
        .map_err(|e| println!("Transfer failed: {}", e));

    assert!(runner.balances.get_balance(&alice) == 70);
    assert!(runner.balances.get_balance(&bob) == 30);

    let _ = runner.balances
        .transfer(&alice, &charlie, 50)
        .map_err(|e| println!("Transfer failed: {}", e));

    assert!(runner.balances.get_balance(&alice) == 20);
    assert!(runner.balances.get_balance(&charlie) == 50);
    assert!(runner.balances.get_balance(&bob) == 30);

    println!("{:?}", runner);
}