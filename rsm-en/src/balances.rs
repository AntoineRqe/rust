use std::{collections::BTreeMap, fmt::Debug};
use std::fmt::Display;
use num::{CheckedAdd, CheckedSub, Zero};

use crate::support::Dispatch;
use crate::system;

pub trait Config : system::Config {
    type Balance: Copy + Display + CheckedAdd + CheckedSub + Zero + Debug;    
}

#[derive(Debug)]
pub struct Pallet<T: Config> {
    balances: BTreeMap<T::AccountId, T::Balance>,
}

pub enum Call<T: Config> {
    Transfer { to: T::AccountId, amount: T::Balance },
}

impl <T: Config> crate::support::Dispatch for Pallet<T> {
    type Caller = T::AccountId;
    type Call = Call<T>;

    fn dispatch(&mut self, caller: Self::Caller, call: Self::Call) -> crate::support::DispatchResult {
        match call {
            Call::Transfer { to, amount } => {
                self.transfer(&caller, &to, amount)?;
            },
        }

        Ok(())
    }
    
}
impl<T: Config> Pallet<T>
{
    pub fn new() -> Self {
        Self {
            balances: BTreeMap::new(),
        }
    }

    pub fn set_balance(&mut self, account: &T::AccountId, amount: T::Balance) {
        self.balances.insert(account.clone(), amount);
    }

    pub fn get_balance(&self, account: &T::AccountId) -> T::Balance {
        *self.balances.get(account).unwrap_or(&T::Balance::zero())
    }

    pub fn transfer(&mut self, from: &T::AccountId, to: &T::AccountId, amount: T::Balance) -> Result<(), &'static str> {
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

    struct TestConfig;

    impl system::Config for TestConfig {
        type AccountId = String;
        type BlockNumber = u32;
        type Nonce = u32;        
    }

    impl super::Config for TestConfig {
        type Balance = u128;
    }

    use super::*;
    #[test]
    fn test_balances() {
        let mut pallet = Pallet::<TestConfig>::new();
        pallet.set_balance(&"Alice".to_string(), 1000);
        assert_eq!(pallet.get_balance(&"Alice".to_string()), 1000);
        assert_eq!(pallet.get_balance(&"Bob".to_string()), 0);
    }

    #[test]
    fn test_transfer() {
        let mut pallet: Pallet<TestConfig> = Pallet::new();
        pallet.set_balance(&"Alice".to_string(), 1000);
        pallet.set_balance(&"Bob".to_string(), 500);
        assert!(pallet.transfer(&"Alice".to_string(), &"Bob".to_string(), 300).is_ok());
        assert_eq!(pallet.get_balance(&"Alice".to_string()), 700);
        assert_eq!(pallet.get_balance(&"Bob".to_string()), 800);
        assert!(pallet.transfer(&"Alice".to_string(), &"Bob".to_string(), 800).is_err());
    }
}