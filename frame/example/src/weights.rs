//! Autogenerated weights for pallet_example
//!
//! THIS FILE WAS AUTO-GENERATED USING THE SUBSTRATE BENCHMARK CLI VERSION 3.0.0
//! DATE: 2021-03-01, STEPS: [50, ], REPEAT: 20, LOW RANGE: [], HIGH RANGE: []
//! EXECUTION: Some(Wasm), WASM-EXECUTION: Compiled, CHAIN: Some("dev"), DB CACHE: 128

// Executed Command:
// ./target/debug/substrate
// benchmark
// --chain
// dev
// --execution
// wasm
// --wasm-execution
// compiled
// --pallet
// pallet_example
// --extrinsic
// *
// --steps
// 50
// --repeat
// 20
// --output
// ./
// --log
// benchmark


#![allow(unused_parens)]
#![allow(unused_imports)]

use frame_support::{traits::Get, weights::{Weight, constants::RocksDbWeight}};
use sp_std::marker::PhantomData;
use crate::WeightInfo;

/// Weight functions for pallet_example.
pub struct SubstrateWeight<T>(PhantomData<T>);
impl<T: frame_system::Config> WeightInfo for SubstrateWeight<T> {
	fn accumulate_dummy(_b: u32, ) -> Weight {
		(2_065_526_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(1 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	fn set_dummy(_b: u32, ) -> Weight {
		(229_850_000 as Weight)
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	fn another_set_dummy(b: u32, ) -> Weight {
		(228_835_000 as Weight)
			// Standard Error: 62_000
			.saturating_add((20_000 as Weight).saturating_mul(b as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	fn sort_vector(x: u32, ) -> Weight {
		(123_304_000 as Weight)
			// Standard Error: 0
			.saturating_add((5_000 as Weight).saturating_mul(x as Weight))
	}
}

impl WeightInfo for () {
	fn accumulate_dummy(_b: u32, ) -> Weight {
		(2_065_526_000 as Weight)
			.saturating_add(RocksDbWeight::get().reads(1 as Weight))
			.saturating_add(RocksDbWeight::get().writes(1 as Weight))
	}
	fn set_dummy(_b: u32, ) -> Weight {
		(229_850_000 as Weight)
			.saturating_add(RocksDbWeight::get().writes(1 as Weight))
	}
	fn another_set_dummy(b: u32, ) -> Weight {
		(228_835_000 as Weight)
			// Standard Error: 62_000
			.saturating_add((20_000 as Weight).saturating_mul(b as Weight))
			.saturating_add(RocksDbWeight::get().writes(1 as Weight))
	}
	fn sort_vector(x: u32, ) -> Weight {
		(123_304_000 as Weight)
			// Standard Error: 0
			.saturating_add((5_000 as Weight).saturating_mul(x as Weight))
	}
}