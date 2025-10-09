use std::{collections::BTreeMap, fmt::Debug};
use std::fmt::Display;
use num::{CheckedAdd, CheckedSub, Zero};

#[derive(Debug)]
pub struct Pallet<AccountId, Balance> {
    balances: BTreeMap<AccountId, Balance>,
}

impl<AccountId, Balance> Pallet<AccountId, Balance>
where
    AccountId: Clone + Ord,
    Balance: Copy + Display + CheckedAdd + CheckedSub + Zero + Debug,
{
    pub fn new() -> Self {
        Self {
            balances: BTreeMap::new(),
        }
    }

    pub fn set_balance(&mut self, account: &AccountId, amount: Balance) {
        self.balances.insert(account.clone(), amount);
    }

    pub fn get_balance(&self, account: &AccountId) -> Balance {
        *self.balances.get(account).unwrap_or(&Balance::zero())
    }

    pub fn transfer(&mut self, from: &AccountId, to: &AccountId, amount: Balance) -> Result<(), &'static str> {
        let from_balance = self.get_balance(from);
        let to_balance = self.get_balance(to);

        let new_from_balance = from_balance
            .checked_sub(&amount)
            .ok_or("Insufficient balance")?;
        
        let new_to_balance = to_balance
            .checked_add(&amount)
            .ok_or("Balance overflow")?;

        self.set_balance(from, new_from_balance);
        self.set_balance(to, new_to_balance);

        Ok(())        
    }
}

#[cfg(test)]
mod tests {
    use crate::types::{AccountId, Balance};

    use super::*;
    #[test]
    fn test_balances() {
        let mut pallet = Pallet::new();
        pallet.set_balance(&"Alice".to_string(), 1000);
        assert_eq!(pallet.get_balance(&"Alice".to_string()), 1000);
        assert_eq!(pallet.get_balance(&"Bob".to_string()), 0);
    }

    #[test]
    fn test_transfer() {
        let mut pallet: Pallet<AccountId, Balance> = Pallet::new();
        pallet.set_balance(&"Alice".to_string(), 1000);
        pallet.set_balance(&"Bob".to_string(), 500);
        assert!(pallet.transfer(&"Alice".to_string(), &"Bob".to_string(), 300).is_ok());
        assert_eq!(pallet.get_balance(&"Alice".to_string()), 700);
        assert_eq!(pallet.get_balance(&"Bob".to_string()), 800);
        assert!(pallet.transfer(&"Alice".to_string(), &"Bob".to_string(), 800).is_err());
    }
}