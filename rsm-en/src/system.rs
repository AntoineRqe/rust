use std::{collections::BTreeMap, ops::AddAssign};
use num::{CheckedAdd, One, Zero};

pub trait Config {
    type AccountId: Clone + Ord;
    type BlockNumber: Copy + AddAssign + Zero + One;
    type Nonce: Copy + CheckedAdd + Zero + One;
} 

#[derive(Debug)]
pub struct Pallet<T: Config> {
    block_number: T::BlockNumber,
    nonce: BTreeMap<T::AccountId, T::Nonce>,
}

impl<T: Config> Pallet<T> {
    pub fn new() -> Self {
        Pallet {
            block_number:   T::BlockNumber::zero(),
            nonce: BTreeMap::new(),
        }
    }

    pub fn get_block_number(&self) -> T::BlockNumber {
        self.block_number
    }

    pub fn get_nonce(&self, account: &T::AccountId) -> T::Nonce {
        self.nonce.get(account).cloned().unwrap_or(T::Nonce::zero())
    }

    pub fn inc_block(&mut self) {
        // Crashes if overflow occurs
        self.block_number += T::BlockNumber::one();
    }

    pub fn inc_nonce(&mut self, account: &T::AccountId) {
        let nonce = *self.nonce.get(account).unwrap_or(&T::Nonce::zero());
        let new_nonce = nonce + T::Nonce::one();
        self.nonce.insert(account.clone(), new_nonce);
    }
}

#[cfg(test)]
mod tests {

    struct TestConfig;

    impl Config for TestConfig {
        type AccountId = String;
        type BlockNumber = u32;
        type Nonce = u32;
    }
    use super::*;

    #[test]
    fn test_init_system() {
        let system: Pallet<TestConfig> = Pallet::new();
        assert_eq!(system.get_block_number(), 0);
    }

    #[test]
    fn test_inc_block_number() 
    {
        let mut system: Pallet<TestConfig> = Pallet::new();
        for i in 1..=10 {
            system.inc_block();
            assert_eq!(system.get_block_number(), i);
        }
    }

    #[test]
    fn test_inc_nonce() {
        let mut system: Pallet<TestConfig> = Pallet::new();
        let account = "Bob".to_string();
        for i in 1..=10 {
            system.inc_nonce(&account);
            assert_eq!(system.get_nonce(&account), i);
        }
    }
}