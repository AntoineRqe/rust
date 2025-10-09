use std::collections::BTreeMap;
use num::{CheckedAdd, One, Zero};

#[derive(Debug)]
pub struct Pallet<AccountId, BlockNumber, Nonce> {
    block_number: BlockNumber,
    nonce: BTreeMap<AccountId, Nonce>,
}

impl<AccountId, BlockNumber, Nonce> Pallet<AccountId, BlockNumber, Nonce>
where
    AccountId: Clone + Ord,
    BlockNumber: Copy + CheckedAdd + Zero + One,
    Nonce: Copy + CheckedAdd + Zero + One,
{
    pub fn new() -> Self {
        Pallet {
            block_number: BlockNumber::zero(),
            nonce: BTreeMap::new(),
        }
    }

    pub fn get_block_number(&self) -> BlockNumber {
        self.block_number
    }

    pub fn get_nonce(&self, account: &AccountId) -> Nonce {
        self.nonce.get(account).cloned().unwrap_or(Nonce::zero())
    }

    pub fn inc_block(&mut self) {
        // Crashes if overflow occurs

        self.block_number = self.block_number
            .checked_add(&BlockNumber::one())
            .expect("Block number overflow");
    }

    pub fn inc_nonce(&mut self, account: &AccountId) {
        let current_nonce = *self.nonce.get(account).unwrap_or(&Nonce::zero());
        let new_nonce = current_nonce
            .checked_add(&Nonce::one())
            .expect("Nonce overflow");
        self.nonce.insert(account.clone(), new_nonce);
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::types::{AccountId, BlockNumber, Nonce};
    #[test]
    fn test_init_system() {
        let system: Pallet<AccountId, BlockNumber, Nonce> = Pallet::new();
        assert_eq!(system.get_block_number(), 0);
    }

    #[test]
    fn test_inc_block_number() 
    {
        let mut system: Pallet<AccountId, BlockNumber, Nonce> = Pallet::new();
        for i in 1..=10 {
            system.inc_block();
            assert_eq!(system.get_block_number(), i);
        }
    }

    #[test]
    fn test_inc_nonce() {
        let mut system: Pallet<AccountId, BlockNumber, Nonce> = Pallet::new();
        let account = "Bob".to_string();
        for i in 1..=10 {
            system.inc_nonce(&account);
            assert_eq!(system.get_nonce(&account), i);
        }
    }
}