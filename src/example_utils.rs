#[cfg(feature = "example_utils")]
pub mod example_utils {
    use revm::db::{CacheDB, Database, DatabaseCommit, DatabaseRef};
    use revm::primitives::{Account, AccountInfo, Address, Bytecode, HashMap, B256, U256};
    use serde::{Deserialize, Serialize};
    use std::convert::Infallible;

    #[derive(Clone, Default, Serialize, Deserialize)]
    pub struct PlaceholderDB;
    impl DatabaseRef for PlaceholderDB {
        type Error = Infallible;

        /// Get basic account information.
        fn basic_ref(&self, _address: Address) -> Result<Option<AccountInfo>, Self::Error> {
            Ok(None)
        }

        /// Get account code by its hash.
        fn code_by_hash_ref(&self, _code_hash: B256) -> Result<Bytecode, Self::Error> {
            unimplemented!(
                "PlaceholderDB: code_by_hash_ref on thread {:?}",
                std::thread::current().id()
            )
        }

        /// Get storage value of address at index.
        fn storage_ref(&self, _address: Address, _index: U256) -> Result<U256, Self::Error> {
            Ok(U256::ZERO)
        }

        /// Get block hash by block number.
        fn block_hash_ref(&self, _number: U256) -> Result<B256, Self::Error> {
            unimplemented!(
                "PlaceholderDB: block_hash_ref on thread {:?}",
                std::thread::current().id()
            )
        }
    }

    pub struct BackedDB<ExtDB> {
        pub base_db: CacheDB<ExtDB>,
        pub db: CacheDB<ExtDB>,
    }

    impl<ExtDB: Clone> BackedDB<ExtDB> {
        pub fn new(db: ExtDB) -> Self {
            Self {
                base_db: CacheDB::new(db.clone()),
                db: CacheDB::new(db.clone()),
            }
        }

        pub fn get_serializable_base_db(&self) -> CacheDB<PlaceholderDB> {
            let mut serde_db = CacheDB::new(PlaceholderDB::default());
            serde_db.accounts = self.base_db.accounts.clone();
            serde_db.contracts = self.base_db.contracts.clone();
            serde_db.logs = self.base_db.logs.clone();
            serde_db.block_hashes = self.base_db.block_hashes.clone();
            serde_db
        }
    }

    impl<ExtDB> DatabaseCommit for BackedDB<ExtDB> {
        fn commit(&mut self, changes: HashMap<Address, Account>) {
            self.db.commit(changes);
        }
    }

    impl<ExtDB: DatabaseRef> Database for BackedDB<ExtDB> {
        type Error = ExtDB::Error;

        /// Get basic account information.
        fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
            let _ = self.base_db.basic(address);
            self.db.basic(address)
        }

        /// Get account code by its hash.
        fn code_by_hash(&mut self, code_hash: B256) -> Result<Bytecode, Self::Error> {
            let _ = self.base_db.code_by_hash(code_hash);
            self.db.code_by_hash(code_hash)
        }

        /// Get storage value of address at index.
        fn storage(&mut self, address: Address, index: U256) -> Result<U256, Self::Error> {
            let _ = self.base_db.storage(address, index);
            self.db.storage(address, index)
        }

        /// Get block hash by block number.
        fn block_hash(&mut self, number: U256) -> Result<B256, Self::Error> {
            let _ = self.base_db.block_hash(number);
            self.db.block_hash(number)
        }
    }
}
