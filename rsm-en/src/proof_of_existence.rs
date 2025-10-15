use core::fmt::Debug;
use std::collections::BTreeMap;

use crate::support::DispatchResult;

pub trait Config: crate::system::Config {
    type Content:  Debug + Ord;
}

#[derive(Debug)]
pub struct Pallet<T: Config> {
    claims: BTreeMap<T::Content, T::AccountId>,
}

pub enum Call<T: Config> {
    CreateClaim { claim: T::Content },
    RevokeClaim { claim: T::Content },
    
}

impl <T: Config> crate::support::Dispatch for Pallet<T> {
    type Caller = T::AccountId;
    type Call = Call<T>;

    fn dispatch(&mut self, caller: Self::Caller, call: Self::Call) -> crate::support::DispatchResult {
        match call {
            Call::CreateClaim { claim } => {
                self.create_claim(caller, claim)?;
            },
            Call::RevokeClaim { claim } => {
                self.revoke_claim(caller, claim)?;
            },
        }

        Ok(())
    }
    
}

impl<T: Config> Pallet<T> {
    pub fn new() -> Self {
        Self {
            claims: BTreeMap::new(),
        }
    }

    pub fn get_claim(&self, content: &T::Content) -> Option<&T::AccountId> {
        self.claims.get(content)
    }

    pub fn create_claim(
        &mut self,
        caller: T::AccountId,
        content: T::Content,
    ) -> DispatchResult {
        if self.claims.contains_key(&content) {
            return Err("Proof already exists");
        }

        self.claims.insert(content, caller);
        Ok(())
    }

    pub fn revoke_claim(
        &mut self,
        caller: T::AccountId,
        content: T::Content,
    ) -> DispatchResult {
        match self.claims.get(&content) {
            Some(owner) if *owner == caller => {
                self.claims.remove(&content);
                Ok(())
            }
            Some(_) => Err("Not the claim owner"),
            None => Err("Claim does not exist"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    struct TestConfig;

    impl Config for TestConfig {
        type Content = &'static str;
    }

    impl crate::system::Config for TestConfig  {
        type BlockNumber = u32;
        type AccountId = &'static str;
        type Nonce = u32; 
    }

    #[test]
    fn test_create_claim() {
        let mut pallet = Pallet::<TestConfig>::new();

        assert!(pallet.create_claim("alice", "my claim").is_ok());
        assert_eq!(pallet.get_claim(&"my claim"), Some(&"alice"));
    }

    #[test]
    fn test_revoke_claim() {
        let mut pallet = Pallet::<TestConfig>::new();


        pallet.create_claim("alice", "my claim").unwrap();
        assert!(pallet.revoke_claim("alice", "my wrong claim").is_err());
        assert!(pallet.revoke_claim("alice", "my claim").is_ok());
        assert_eq!(pallet.get_claim(&"my claim"), None);
    }

}
