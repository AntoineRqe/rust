use std::collections::BTreeMap;
pub struct Pallet {
    balances: BTreeMap<String, u128>,
}

impl Pallet {
    pub fn new() -> Self {
        Self {
            balances: BTreeMap::new(),
        }
    }

    pub fn set_balance(&mut self, account: String, amount: u128) {
        self.balances.insert(account, amount);
    }

    pub fn get_balance(&self, account: &String) -> &u128 {
        self.balances.get(account).unwrap_or(&0)
    }

    pub fn transfer(&mut self, from: &String, to: &String, amount: u128) -> Result<(), String> {
        let from_balance = self.get_balance(from);
        let to_balance = self.get_balance(to);
    
        let new_from_balance = from_balance
            .checked_sub(amount)
            .ok_or("Insufficient balance".to_string())?;
        
        let new_to_balance = to_balance
            .checked_add(amount)
            .ok_or("Balance overflow".to_string())?;

        self.set_balance(from.clone(), new_from_balance);
        self.set_balance(to.clone(), new_to_balance);

        Ok(())        
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_balances() {
        let mut pallet = Pallet::new();
        pallet.set_balance("Alice".to_string(), 1000);
        assert_eq!(*pallet.get_balance(&"Alice".to_string()), 1000);
        assert_eq!(*pallet.get_balance(&"Bob".to_string()), 0);
    }

    #[test]
    fn test_transfer() {
        let mut pallet = Pallet::new();
        pallet.set_balance("Alice".to_string(), 1000);
        pallet.set_balance("Bob".to_string(), 500);
        assert!(pallet.transfer(&"Alice".to_string(), &"Bob".to_string(), 300).is_ok());
        assert_eq!(*pallet.get_balance(&"Alice".to_string()), 700);
        assert_eq!(*pallet.get_balance(&"Bob".to_string()), 800);
        assert!(pallet.transfer(&"Alice".to_string(), &"Bob".to_string(), 800).is_err());
    }
}