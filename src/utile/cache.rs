use alloy::primitives::{Address, U256};
use dashmap::DashMap;
use fxhash::FxHasher;
use std::hash::{BuildHasherDefault, Hash, Hasher};

/// Custom hasher based on `FxHasher` (fast non-cryptographic hashing)
#[derive(Default)]
struct CacheHasher(FxHasher);

impl Hasher for CacheHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.0.finish()
    }

    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        self.0.write(bytes)
    }
}

/// Composite key to cache a specific pool's quote with an exact input amount
#[derive(PartialEq, Eq, Clone, Copy, Debug, Hash)]
struct CacheKey {
    pub pool_address: Address,
    pub amount_in: U256,
}

/// Represents a single output entry from a simulation or estimation
#[derive(Clone, Copy, Debug)]
struct CacheEntry {
    pub output_amount: U256,
}

/// A concurrent, fast read/write cache for pool simulations and estimations
pub struct Cache {
    entries: DashMap<CacheKey, CacheEntry, BuildHasherDefault<CacheHasher>>,
}

impl Cache {
    /// Construct a new cache sized based on the expected number of pools.
    /// We estimate 100 input variations per pool to preallocate capacity.
    pub fn new(num_pools: usize) -> Self {
        Self {
            entries: DashMap::with_capacity_and_hasher(
                num_pools * 100,
                BuildHasherDefault::default(),
            ),
        }
    }

    /// Retrieves a cached output amount for a given pool + input amount.
    #[inline]
    pub fn get(&self, amount_in: U256, pool_address: Address) -> Option<U256> {
        let key = CacheKey {
            pool_address,
            amount_in,
        };
        match self.entries.get(&key) {
            Some(entry) => Some(entry.output_amount),
            None => None,
        }
    }

    /// Stores a new output amount in the cache
    #[inline]
    pub fn insert(&self, amount_in: U256, pool_address: Address, output_amount: U256) {
        let key = CacheKey {
            pool_address,
            amount_in,
        };
        self.entries.insert(key, CacheEntry { output_amount });
    }

    /// Invalidate all cache entries for a given pool
    #[inline]
    pub fn invalidate(&self, pool_address: Address) {
        self.entries
            .retain(|key, _| key.pool_address != pool_address);
    }

    /// Clears all entries in the cache
    #[inline]
    pub fn clear(&self) {
        self.entries.clear();
    }

    /// Total entries in the cache
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Checks if the cache is empty
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
