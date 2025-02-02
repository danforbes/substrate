// This file is part of Substrate.

// Copyright (C) 2018-2021 Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// 	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! # Contract Module
//!
//! The Contract module provides functionality for the runtime to deploy and execute WebAssembly smart-contracts.
//!
//! - [`contract::Config`](./trait.Config.html)
//! - [`Call`](./enum.Call.html)
//!
//! ## Overview
//!
//! This module extends accounts based on the `Currency` trait to have smart-contract functionality. It can
//! be used with other modules that implement accounts based on `Currency`. These "smart-contract accounts"
//! have the ability to instantiate smart-contracts and make calls to other contract and non-contract accounts.
//!
//! The smart-contract code is stored once in a `code_cache`, and later retrievable via its `code_hash`.
//! This means that multiple smart-contracts can be instantiated from the same `code_cache`, without replicating
//! the code each time.
//!
//! When a smart-contract is called, its associated code is retrieved via the code hash and gets executed.
//! This call can alter the storage entries of the smart-contract account, instantiate new smart-contracts,
//! or call other smart-contracts.
//!
//! Finally, when an account is reaped, its associated code and storage of the smart-contract account
//! will also be deleted.
//!
//! ### Gas
//!
//! Senders must specify a gas limit with every call, as all instructions invoked by the smart-contract require gas.
//! Unused gas is refunded after the call, regardless of the execution outcome.
//!
//! If the gas limit is reached, then all calls and state changes (including balance transfers) are only
//! reverted at the current call's contract level. For example, if contract A calls B and B runs out of gas mid-call,
//! then all of B's calls are reverted. Assuming correct error handling by contract A, A's other calls and state
//! changes still persist.
//!
//! ### Notable Scenarios
//!
//! Contract call failures are not always cascading. When failures occur in a sub-call, they do not "bubble up",
//! and the call will only revert at the specific contract level. For example, if contract A calls contract B, and B
//! fails, A can decide how to handle that failure, either proceeding or reverting A's changes.
//!
//! ## Interface
//!
//! ### Dispatchable functions
//!
//! * `instantiate_with_code` - Deploys a new contract from the supplied wasm binary, optionally transferring
//! some balance. This instantiates a new smart contract account and calls its contract deploy
//! handler to initialize the contract.
//! * `instantiate` - The same as `instantiate_with_code` but instead of uploading new code an
//! existing `code_hash` is supplied.
//! * `call` - Makes a call to an account, optionally transferring some balance.
//!
//! ## Usage
//!
//! The Contract module is a work in progress. The following examples show how this Contract module
//! can be used to instantiate and call contracts.
//!
//! * [`ink`](https://github.com/paritytech/ink) is
//! an [`eDSL`](https://wiki.haskell.org/Embedded_domain_specific_language) that enables writing
//! WebAssembly based smart contracts in the Rust programming language. This is a work in progress.
//!
//! ## Related Modules
//!
//! * [Balances](../pallet_balances/index.html)

#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(feature = "runtime-benchmarks", recursion_limit="512")]

#[macro_use]
mod gas;
mod storage;
mod exec;
mod wasm;
mod rent;
mod benchmarking;
mod schedule;
mod migration;

pub mod chain_extension;
pub mod weights;

#[cfg(test)]
mod tests;

pub use crate::{
	wasm::PrefabWasmModule,
	schedule::{Schedule, HostFnWeights, InstructionWeights, Limits},
	pallet::*,
};
use crate::{
	gas::GasMeter,
	exec::{ExecutionContext, Executable},
	rent::Rent,
	storage::{Storage, DeletedContract},
	weights::WeightInfo,
};
use sp_core::crypto::UncheckedFrom;
use sp_std::{prelude::*, marker::PhantomData, fmt::Debug};
use codec::{Codec, Encode, Decode};
use sp_runtime::{
	traits::{
		Hash, StaticLookup, MaybeSerializeDeserialize, Member, Convert, Saturating, Zero,
	},
	RuntimeDebug, Perbill,
};
use frame_support::{
	storage::child::ChildInfo,
	traits::{OnUnbalanced, Currency, Get, Time, Randomness},
	weights::{Weight, PostDispatchInfo, WithPostDispatchInfo},
};
use frame_system::Module as System;
use pallet_contracts_primitives::{
	RentProjectionResult, GetStorageResult, ContractAccessError, ContractExecResult,
};

pub type CodeHash<T> = <T as frame_system::Config>::Hash;
pub type TrieId = Vec<u8>;
pub type BalanceOf<T> =
	<<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;
pub type NegativeImbalanceOf<T> =
	<<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::NegativeImbalance;
pub type AliveContractInfo<T> =
	RawAliveContractInfo<CodeHash<T>, BalanceOf<T>, <T as frame_system::Config>::BlockNumber>;
pub type TombstoneContractInfo<T> =
	RawTombstoneContractInfo<<T as frame_system::Config>::Hash, <T as frame_system::Config>::Hashing>;

#[frame_support::pallet]
pub mod pallet {
	use frame_support::pallet_prelude::*;
	use frame_system::pallet_prelude::*;
	use super::*;

	/// Used to answer contracts' queries regarding the current weight price. This is **not**
	/// used to calculate the actual fee and is only for informational purposes.
	type WeightPrice: Convert<Weight, BalanceOf<Self>>;
	#[pallet::config]
	pub trait Config: frame_system::Config {
		/// The time implementation used to supply timestamps to conntracts through `seal_now`.
		type Time: Time;

		/// The generator used to supply randomness to contracts through `seal_random`.
		type Randomness: Randomness<Self::Hash, Self::BlockNumber>;

		/// The currency in which fees are paid and contract balances are held.
		type Currency: Currency<Self::AccountId>;

		/// The overarching event type.
		type Event: From<Event<Self>> + IsType<<Self as frame_system::Config>::Event>;

		/// Handler for rent payments.
		type RentPayment: OnUnbalanced<NegativeImbalanceOf<Self>>;

		/// Number of block delay an extrinsic claim surcharge has.
		///
		/// When claim surcharge is called by an extrinsic the rent is checked
		/// for current_block - delay
		#[pallet::constant]
		type SignedClaimHandicap: Get<Self::BlockNumber>;

		/// The minimum amount required to generate a tombstone.
		#[pallet::constant]
		type TombstoneDeposit: Get<BalanceOf<Self>>;

		/// The balance every contract needs to deposit to stay alive indefinitely.
		///
		/// This is different from the [`Self::TombstoneDeposit`] because this only needs to be
		/// deposited while the contract is alive. Costs for additional storage are added to
		/// this base cost.
		///
		/// This is a simple way to ensure that contracts with empty storage eventually get deleted by
		/// making them pay rent. This creates an incentive to remove them early in order to save rent.
		#[pallet::constant]
		type DepositPerContract: Get<BalanceOf<Self>>;

		/// The balance a contract needs to deposit per storage byte to stay alive indefinitely.
		///
		/// Let's suppose the deposit is 1,000 BU (balance units)/byte and the rent is 1 BU/byte/day,
		/// then a contract with 1,000,000 BU that uses 1,000 bytes of storage would pay no rent.
		/// But if the balance reduced to 500,000 BU and the storage stayed the same at 1,000,
		/// then it would pay 500 BU/day.
		#[pallet::constant]
		type DepositPerStorageByte: Get<BalanceOf<Self>>;

		/// The balance a contract needs to deposit per storage item to stay alive indefinitely.
		///
		/// It works the same as [`Self::DepositPerStorageByte`] but for storage items.
		#[pallet::constant]
		type DepositPerStorageItem: Get<BalanceOf<Self>>;

		/// The fraction of the deposit that should be used as rent per block.
		///
		/// When a contract hasn't enough balance deposited to stay alive indefinitely it needs
		/// to pay per block for the storage it consumes that is not covered by the deposit.
		/// This determines how high this rent payment is per block as a fraction of the deposit.
		#[pallet::constant]
		type RentFraction: Get<Perbill>;

		/// Reward that is received by the party whose touch has led
		/// to removal of a contract.
		#[pallet::constant]
		type SurchargeReward: Get<BalanceOf<Self>>;

		/// The maximum nesting level of a call/instantiate stack.
		#[pallet::constant]
		type MaxDepth: Get<u32>;

		/// The maximum size of a storage value and event payload in bytes.
		#[pallet::constant]
		type MaxValueSize: Get<u32>;

		/// Used to answer contracts's queries regarding the current weight price. This is **not**
		/// used to calculate the actual fee and is only for informational purposes.
		type WeightPrice: Convert<Weight, BalanceOf<Self>>;

		/// Describes the weights of the dispatchables of this module and is also used to
		/// construct a default cost schedule.
		type WeightInfo: WeightInfo;

		/// Type that allows the runtime authors to add new host functions for a contract to call.
		type ChainExtension: chain_extension::ChainExtension<Self>;

		/// The maximum number of tries that can be queued for deletion.
		#[pallet::constant]
		type DeletionQueueDepth: Get<u32>;

		/// The maximum amount of weight that can be consumed per block for lazy trie removal.
		#[pallet::constant]
		type DeletionWeightLimit: Get<Weight>;

		/// The maximum length of a contract code in bytes. This limit applies to the instrumented
		/// version of the code. Therefore `instantiate_with_code` can fail even when supplying
		/// a wasm binary below this maximum size.
		#[pallet::constant]
		type MaxCodeSize: Get<u32>;
	}

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	pub struct Pallet<T>(PhantomData<T>);

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T>
	where
		T::AccountId: UncheckedFrom<T::Hash>,
		T::AccountId: AsRef<[u8]>,
	{
		fn on_initialize(_block: T::BlockNumber) -> Weight {
			// We do not want to go above the block limit and rather avoid lazy deletion
			// in that case. This should only happen on runtime upgrades.
			let weight_limit = T::BlockWeights::get().max_block
				.saturating_sub(System::<T>::block_weight().total())
				.min(T::DeletionWeightLimit::get());
			Storage::<T>::process_deletion_queue_batch(weight_limit)
				.saturating_add(T::WeightInfo::on_initialize())
		}

		fn on_runtime_upgrade() -> Weight {
			migration::migrate::<T>()
		}
	}

	#[pallet::call]
	impl<T: Config> Pallet<T>
	where
		T::AccountId: UncheckedFrom<T::Hash>,
		T::AccountId: AsRef<[u8]>,
	{
		/// Updates the schedule for metering contracts.
		///
		/// The schedule's version cannot be less than the version of the stored schedule.
		/// If a schedule does not change the instruction weights the version does not
		/// need to be increased. Therefore we allow storing a schedule that has the same
		/// version as the stored one.
		#[pallet::weight(T::WeightInfo::update_schedule())]
		pub fn update_schedule(
			origin: OriginFor<T>,
			schedule: Schedule<T>
		) -> DispatchResultWithPostInfo {
			ensure_root(origin)?;
			if <Module<T>>::current_schedule().version > schedule.version {
				Err(Error::<T>::InvalidScheduleVersion)?
			}
			Self::deposit_event(Event::ScheduleUpdated(schedule.version));
			CurrentSchedule::put(schedule);
			Ok(().into())
		}

		/// Makes a call to an account, optionally transferring some balance.
		///
		/// * If the account is a smart-contract account, the associated code will be
		/// executed and any value will be transferred.
		/// * If the account is a regular account, any value will be transferred.
		/// * If no account exists and the call value is not less than `existential_deposit`,
		/// a regular account will be created and any value will be transferred.
		#[pallet::weight(T::WeightInfo::call(T::MaxCodeSize::get() / 1024).saturating_add(*gas_limit))]
		pub fn call(
			origin: OriginFor<T>,
			dest: <T::Lookup as StaticLookup>::Source,
			#[pallet::compact] value: BalanceOf<T>,
			#[pallet::compact] gas_limit: Weight,
			data: Vec<u8>
		) -> DispatchResultWithPostInfo {
			let origin = ensure_signed(origin)?;
			let dest = T::Lookup::lookup(dest)?;
			let mut gas_meter = GasMeter::new(gas_limit);
			let schedule = <Module<T>>::current_schedule();
			let mut ctx = ExecutionContext::<T, PrefabWasmModule<T>>::top_level(origin, &schedule);
			let (result, code_len) = match ctx.call(dest, value, &mut gas_meter, data) {
				Ok((output, len)) => (Ok(output), len),
				Err((err, len)) => (Err(err), len),
			};
			gas_meter.into_dispatch_result(result, T::WeightInfo::call(code_len / 1024))
		}

		/// Instantiates a new contract from the supplied `code` optionally transferring
		/// some balance.
		///
		/// This is the only function that can deploy new code to the chain.
		///
		/// # Parameters
		///
		/// * `endowment`: The balance to transfer from the `origin` to the newly created contract.
		/// * `gas_limit`: The gas limit enforced when executing the constructor.
		/// * `code`: The contract code to deploy in raw bytes.
		/// * `data`: The input data to pass to the contract constructor.
		/// * `salt`: Used for the address derivation. See [`Self::contract_address`].
		///
		/// Instantiation is executed as follows:
		///
		/// - The supplied `code` is instrumented, deployed, and a `code_hash` is created for that code.
		/// - If the `code_hash` already exists on the chain the underlying `code` will be shared.
		/// - The destination address is computed based on the sender, code_hash and the salt.
		/// - The smart-contract account is created at the computed address.
		/// - The `endowment` is transferred to the new account.
		/// - The `deploy` function is executed in the context of the newly-created account.
		#[pallet::weight(
			T::WeightInfo::instantiate_with_code(
				code.len() as u32 / 1024,
				salt.len() as u32 / 1024,
			)
			.saturating_add(*gas_limit)
		)]
		pub fn instantiate_with_code(
			origin: OriginFor<T>,
			#[pallet::compact] endowment: BalanceOf<T>,
			#[pallet::compact] gas_limit: Weight,
			code: Vec<u8>,
			data: Vec<u8>,
			salt: Vec<u8>,
		) -> DispatchResultWithPostInfo {
			let origin = ensure_signed(origin)?;
			let code_len = code.len() as u32;
			ensure!(code_len <= T::MaxCodeSize::get(), Error::<T>::CodeTooLarge);
			let mut gas_meter = GasMeter::new(gas_limit);
			let schedule = <Module<T>>::current_schedule();
			let executable = PrefabWasmModule::from_code(code, &schedule)?;
			let code_len = executable.code_len();
			ensure!(code_len <= T::MaxCodeSize::get(), Error::<T>::CodeTooLarge);
			let mut ctx = ExecutionContext::<T, PrefabWasmModule<T>>::top_level(origin, &schedule);
			let result = ctx.instantiate(endowment, &mut gas_meter, executable, data, &salt)
				.map(|(_address, output)| output);
			gas_meter.into_dispatch_result(
				result,
				T::WeightInfo::instantiate_with_code(code_len / 1024, salt.len() as u32 / 1024)
			)
		}

		/// Instantiates a contract from a previously deployed wasm binary.
		///
		/// This function is identical to [`Self::instantiate_with_code`] but without the
		/// code deployment step. Instead, the `code_hash` of an on-chain deployed wasm binary
		/// must be supplied.
		#[pallet::weight(
			T::WeightInfo::instantiate(T::MaxCodeSize::get() / 1024, salt.len() as u32 / 1024)
				.saturating_add(*gas_limit)
		)]
		pub fn instantiate(
			origin: OriginFor<T>,
			#[pallet::compact] endowment: BalanceOf<T>,
			#[pallet::compact] gas_limit: Weight,
			code_hash: CodeHash<T>,
			data: Vec<u8>,
			salt: Vec<u8>,
		) -> DispatchResultWithPostInfo {
			let origin = ensure_signed(origin)?;
			let mut gas_meter = GasMeter::new(gas_limit);
			let schedule = <Module<T>>::current_schedule();
			let executable = PrefabWasmModule::from_storage(code_hash, &schedule, &mut gas_meter)?;
			let mut ctx = ExecutionContext::<T, PrefabWasmModule<T>>::top_level(origin, &schedule);
			let code_len = executable.code_len();
			let result = ctx.instantiate(endowment, &mut gas_meter, executable, data, &salt)
				.map(|(_address, output)| output);
			gas_meter.into_dispatch_result(
				result,
				T::WeightInfo::instantiate(code_len / 1024, salt.len() as u32 / 1024),
			)
		}

		/// Allows block producers to claim a small reward for evicting a contract. If a block
		/// producer fails to do so, a regular users will be allowed to claim the reward.
		///
		/// In case of a successful eviction no fees are charged from the sender. However, the
		/// reward is capped by the total amount of rent that was payed by the contract while
		/// it was alive.
		///
		/// If contract is not evicted as a result of this call, [`Error::ContractNotEvictable`]
		/// is returned and the sender is not eligible for the reward.
		#[pallet::weight(T::WeightInfo::claim_surcharge(T::MaxCodeSize::get() / 1024))]
		pub fn claim_surcharge(
			origin: OriginFor<T>,
			dest: T::AccountId,
			aux_sender: Option<T::AccountId>
		) -> DispatchResultWithPostInfo {
			let origin = origin.into();
			let (signed, rewarded) = match (origin, aux_sender) {
				(Ok(frame_system::RawOrigin::Signed(account)), None) => {
					(true, account)
				},
				(Ok(frame_system::RawOrigin::None), Some(aux_sender)) => {
					(false, aux_sender)
				},
				_ => Err(Error::<T>::InvalidSurchargeClaim)?,
			};

			// Add some advantage for block producers (who send unsigned extrinsics) by
			// adding a handicap: for signed extrinsics we use a slightly older block number
			// for the eviction check. This can be viewed as if we pushed regular users back in past.
			let handicap = if signed {
				T::SignedClaimHandicap::get()
			} else {
				Zero::zero()
			};

			// If poking the contract has lead to eviction of the contract, give out the rewards.
			match Rent::<T, PrefabWasmModule<T>>::try_eviction(&dest, handicap)? {
				(Some(rent_payed), code_len) => {
					T::Currency::deposit_into_existing(
						&rewarded,
						T::SurchargeReward::get().min(rent_payed),
					)
					.map(|_| PostDispatchInfo {
						actual_weight: Some(T::WeightInfo::claim_surcharge(code_len / 1024)),
						pays_fee: Pays::No,
					})
					.map_err(Into::into)
				}
				(None, code_len) => Err(Error::<T>::ContractNotEvictable.with_weight(
					T::WeightInfo::claim_surcharge(code_len / 1024)
				)),
			}
		}
	}

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	#[pallet::metadata(T::AccountId = "AccountId", T::Hash = "Hash", BalanceOf<T> = "Balance")]
	pub enum Event<T: Config> {
		/// Contract deployed by address at the specified address. \[deployer, contract\]
		Instantiated(T::AccountId, T::AccountId),

		/// Contract has been evicted and is now in tombstone state. \[contract\]
		Evicted(T::AccountId),

		/// Contract has been terminated without leaving a tombstone.
		/// \[contract, beneficiary\]
		///
		/// # Params
		///
		/// - `contract`: The contract that was terminated.
		/// - `beneficiary`: The account that received the contracts remaining balance.
		///
		/// # Note
		///
		/// The only way for a contract to be removed without a tombstone and emitting
		/// this event is by calling `seal_terminate`.
		Terminated(T::AccountId, T::AccountId),

		/// Restoration of a contract has been successful.
		/// \[restorer, dest, code_hash, rent_allowance\]
		///
		/// # Params
		///
		/// - `restorer`: Account ID of the restoring contract.
		/// - `dest`: Account ID of the restored contract.
		/// - `code_hash`: Code hash of the restored contract.
		/// - `rent_allowance`: Rent allowance of the restored contract.
		Restored(T::AccountId, T::AccountId, T::Hash, BalanceOf<T>),

		/// Code with the specified hash has been stored. \[code_hash\]
		CodeStored(T::Hash),

		/// Triggered when the current schedule is updated.
		/// \[version\]
		///
		/// # Params
		///
		/// - `version`: The version of the newly set schedule.
		ScheduleUpdated(u32),

		/// A custom event emitted by the contract.
		/// \[contract, data\]
		///
		/// # Params
		///
		/// - `contract`: The contract that emitted the event.
		/// - `data`: Data supplied by the contract. Metadata generated during contract
		///           compilation is needed to decode it.
		ContractEmitted(T::AccountId, Vec<u8>),

		/// A code with the specified hash was removed.
		/// \[code_hash\]
		///
		/// This happens when the last contract that uses this code hash was removed or evicted.
		CodeRemoved(T::Hash),
	}

	#[pallet::error]
	pub enum Error<T> {
		/// A new schedule must have a greater version than the current one.
		InvalidScheduleVersion,
		/// An origin must be signed or inherent and auxiliary sender only provided on inherent.
		InvalidSurchargeClaim,
		/// Cannot restore from nonexisting or tombstone contract.
		InvalidSourceContract,
		/// Cannot restore to nonexisting or alive contract.
		InvalidDestinationContract,
		/// Tombstones don't match.
		InvalidTombstone,
		/// An origin TrieId written in the current block.
		InvalidContractOrigin,
		/// The executed contract exhausted its gas limit.
		OutOfGas,
		/// The output buffer supplied to a contract API call was too small.
		OutputBufferTooSmall,
		/// Performing the requested transfer would have brought the contract below
		/// the subsistence threshold. No transfer is allowed to do this in order to allow
		/// for a tombstone to be created. Use `seal_terminate` to remove a contract without
		/// leaving a tombstone behind.
		BelowSubsistenceThreshold,
		/// The newly created contract is below the subsistence threshold after executing
		/// its contructor. No contracts are allowed to exist below that threshold.
		NewContractNotFunded,
		/// Performing the requested transfer failed for a reason originating in the
		/// chosen currency implementation of the runtime. Most probably the balance is
		/// too low or locks are placed on it.
		TransferFailed,
		/// Performing a call was denied because the calling depth reached the limit
		/// of what is specified in the schedule.
		MaxCallDepthReached,
		/// The contract that was called is either no contract at all (a plain account)
		/// or is a tombstone.
		NotCallable,
		/// The code supplied to `instantiate_with_code` exceeds the limit specified in the
		/// current schedule.
		CodeTooLarge,
		/// No code could be found at the supplied code hash.
		CodeNotFound,
		/// A buffer outside of sandbox memory was passed to a contract API function.
		OutOfBounds,
		/// Input passed to a contract API function failed to decode as expected type.
		DecodingFailed,
		/// Contract trapped during execution.
		ContractTrapped,
		/// The size defined in `T::MaxValueSize` was exceeded.
		ValueTooLarge,
		/// The action performed is not allowed while the contract performing it is already
		/// on the call stack. Those actions are contract self destruction and restoration
		/// of a tombstone.
		ReentranceDenied,
		/// `seal_input` was called twice from the same contract execution context.
		InputAlreadyRead,
		/// The subject passed to `seal_random` exceeds the limit.
		RandomSubjectTooLong,
		/// The amount of topics passed to `seal_deposit_events` exceeds the limit.
		TooManyTopics,
		/// The topics passed to `seal_deposit_events` contains at least one duplicate.
		DuplicateTopics,
		/// The chain does not provide a chain extension. Calling the chain extension results
		/// in this error. Note that this usually  shouldn't happen as deploying such contracts
		/// is rejected.
		NoChainExtension,
		/// Removal of a contract failed because the deletion queue is full.
		///
		/// This can happen when either calling [`Pallet::claim_surcharge`] or `seal_terminate`.
		/// The queue is filled by deleting contracts and emptied by a fixed amount each block.
		/// Trying again during another block is the only way to resolve this issue.
		DeletionQueueFull,
		/// A contract could not be evicted because it has enough balance to pay rent.
		///
		/// This can be returned from [`Pallet::claim_surcharge`] because the target
		/// contract has enough balance to pay for its rent.
		ContractNotEvictable,
		/// A storage modification exhausted the 32bit type that holds the storage size.
		///
		/// This can either happen when the accumulated storage in bytes is too large or
		/// when number of storage items is too large.
		StorageExhausted,
		/// A contract with the same AccountId already exists.
		DuplicateContract,
	}

	/// Current cost schedule for contracts.
	#[pallet::storage]
	#[pallet::getter(fn current_schedule)]
	pub(super) type CurrentSchedule<T: Config> = StorageValue<_, Schedule<T>, ValueQuery>;

	/// A mapping from an original code hash to the original code, untouched by instrumentation.
	#[pallet::storage]
	pub type PristineCode<T: Config> = StorageMap<_, Identity, CodeHash<T>, Vec<u8>>;

	/// A mapping between an original code hash and instrumented wasm code, ready for execution.
	#[pallet::storage]
	pub type CodeStorage<T: Config> = StorageMap<_, Identity, CodeHash<T>, PrefabWasmModule<T>>;

	/// The subtrie counter.
	#[pallet::storage]
	pub type AccountCounter<T: Config> = StorageValue<_, u64, ValueQuery>;

	/// The code associated with a given account.
	///
	/// TWOX-NOTE: SAFE since `AccountId` is a secure hash.
	#[pallet::storage]
	pub type ContractInfoOf<T: Config> = StorageMap<_, Twox64Concat, T::AccountId, ContractInfo<T>>;

	/// Evicted contracts that await child trie deletion.
	///
	/// Child trie deletion is a heavy operation depending on the amount of storage items
	/// stored in said trie. Therefore this operation is performed lazily in `on_initialize`.
	#[pallet::storage]
	pub type DeletionQueue<T: Config> = StorageValue<_, Vec<DeletedContract>, ValueQuery>;

	#[pallet::genesis_config]
	pub struct GenesisConfig<T: Config> {
		#[doc = "Current cost schedule for contracts."]
		pub current_schedule: Schedule<T>,
	}

	#[cfg(feature = "std")]
	impl<T: Config> Default for GenesisConfig<T> {
		fn default() -> Self {
			Self {
				current_schedule: Default::default(),
			}
		}
	}

	#[pallet::genesis_build]
	impl<T: Config> GenesisBuild<T> for GenesisConfig<T> {
		fn build(&self) {
			<CurrentSchedule<T>>::put(&self.current_schedule);
		}
	}
}

impl<T: Config> Module<T>
where
	T::AccountId: UncheckedFrom<T::Hash> + AsRef<[u8]>,
{
	/// Perform a call to a specified contract.
	///
	/// This function is similar to `Self::call`, but doesn't perform any address lookups and better
	/// suitable for calling directly from Rust.
	///
	/// It returns the execution result and the amount of used weight.
	pub fn bare_call(
		origin: T::AccountId,
		dest: T::AccountId,
		value: BalanceOf<T>,
		gas_limit: Weight,
		input_data: Vec<u8>,
	) -> ContractExecResult {
		let mut gas_meter = GasMeter::new(gas_limit);
		let schedule = <Module<T>>::current_schedule();
		let mut ctx = ExecutionContext::<T, PrefabWasmModule<T>>::top_level(origin, &schedule);
		let result = ctx.call(dest, value, &mut gas_meter, input_data);
		let gas_consumed = gas_meter.gas_spent();
		ContractExecResult {
			exec_result: result.map(|r| r.0).map_err(|r| r.0),
			gas_consumed,
		}
	}

	/// Query storage of a specified contract under a specified key.
	pub fn get_storage(address: T::AccountId, key: [u8; 32]) -> GetStorageResult {
		let contract_info = ContractInfoOf::<T>::get(&address)
			.ok_or(ContractAccessError::DoesntExist)?
			.get_alive()
			.ok_or(ContractAccessError::IsTombstone)?;

		let maybe_value = Storage::<T>::read(&contract_info.trie_id, &key);
		Ok(maybe_value)
	}

	/// Query how many blocks the contract stays alive given that the amount endowment
	/// and consumed storage does not change.
	pub fn rent_projection(address: T::AccountId) -> RentProjectionResult<T::BlockNumber> {
		Rent::<T, PrefabWasmModule<T>>::compute_projection(&address)
	}

	/// Determine the address of a contract,
	///
	/// This is the address generation function used by contract instantiation. Its result
	/// is only dependend on its inputs. It can therefore be used to reliably predict the
	/// address of a contract. This is akin to the formular of eth's CREATE2 opcode. There
	/// is no CREATE equivalent because CREATE2 is strictly more powerful.
	///
	/// Formula: `hash(deploying_address ++ code_hash ++ salt)`
	pub fn contract_address(
		deploying_address: &T::AccountId,
		code_hash: &CodeHash<T>,
		salt: &[u8],
	) -> T::AccountId
	{
		let buf: Vec<_> = deploying_address.as_ref().iter()
			.chain(code_hash.as_ref())
			.chain(salt)
			.cloned()
			.collect();
		UncheckedFrom::unchecked_from(T::Hashing::hash(&buf))
	}

	/// Subsistence threshold is the extension of the minimum balance (aka existential deposit)
	/// by the tombstone deposit, required for leaving a tombstone.
	///
	/// Rent or any contract initiated balance transfer mechanism cannot make the balance lower
	/// than the subsistence threshold in order to guarantee that a tombstone is created.
	///
	/// The only way to completely kill a contract without a tombstone is calling `seal_terminate`.
	pub fn subsistence_threshold() -> BalanceOf<T> {
		T::Currency::minimum_balance().saturating_add(T::TombstoneDeposit::get())
	}

	/// Store code for benchmarks which does not check nor instrument the code.
	#[cfg(feature = "runtime-benchmarks")]
	fn store_code_raw(code: Vec<u8>) -> frame_support::dispatch::DispatchResult {
		let schedule = <Module<T>>::current_schedule();
		PrefabWasmModule::store_code_unchecked(code, &schedule)?;
		Ok(())
	}

	/// This exists so that benchmarks can determine the weight of running an instrumentation.
	#[cfg(feature = "runtime-benchmarks")]
	fn reinstrument_module(
		module: &mut PrefabWasmModule<T>,
		schedule: &Schedule<T>
	) -> frame_support::dispatch::DispatchResult {
		self::wasm::reinstrument(module, schedule)
	}
}

/// Information for managing an account and its sub trie abstraction.
/// This is the required info to cache for an account
#[derive(Encode, Decode, RuntimeDebug)]
pub enum ContractInfo<T: Config> {
	Alive(AliveContractInfo<T>),
	Tombstone(TombstoneContractInfo<T>),
}

impl<T: Config> ContractInfo<T> {
	/// If contract is alive then return some alive info
	pub fn get_alive(self) -> Option<AliveContractInfo<T>> {
		if let ContractInfo::Alive(alive) = self {
			Some(alive)
		} else {
			None
		}
	}
	/// If contract is alive then return some reference to alive info
	pub fn as_alive(&self) -> Option<&AliveContractInfo<T>> {
		if let ContractInfo::Alive(ref alive) = self {
			Some(alive)
		} else {
			None
		}
	}
	/// If contract is alive then return some mutable reference to alive info
	pub fn as_alive_mut(&mut self) -> Option<&mut AliveContractInfo<T>> {
		if let ContractInfo::Alive(ref mut alive) = self {
			Some(alive)
		} else {
			None
		}
	}

	/// If contract is tombstone then return some tombstone info
	pub fn get_tombstone(self) -> Option<TombstoneContractInfo<T>> {
		if let ContractInfo::Tombstone(tombstone) = self {
			Some(tombstone)
		} else {
			None
		}
	}
	/// If contract is tombstone then return some reference to tombstone info
	pub fn as_tombstone(&self) -> Option<&TombstoneContractInfo<T>> {
		if let ContractInfo::Tombstone(ref tombstone) = self {
			Some(tombstone)
		} else {
			None
		}
	}
	/// If contract is tombstone then return some mutable reference to tombstone info
	pub fn as_tombstone_mut(&mut self) -> Option<&mut TombstoneContractInfo<T>> {
		if let ContractInfo::Tombstone(ref mut tombstone) = self {
			Some(tombstone)
		} else {
			None
		}
	}
}

/// Information for managing an account and its sub trie abstraction.
/// This is the required info to cache for an account.
#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug)]
pub struct RawAliveContractInfo<CodeHash, Balance, BlockNumber> {
	/// Unique ID for the subtree encoded as a bytes vector.
	pub trie_id: TrieId,
	/// The total number of bytes used by this contract.
	///
	/// It is a sum of each key-value pair stored by this contract.
	pub storage_size: u32,
	/// The total number of key-value pairs in storage of this contract.
	pub pair_count: u32,
	/// The code associated with a given account.
	pub code_hash: CodeHash,
	/// Pay rent at most up to this value.
	pub rent_allowance: Balance,
	/// The amount of rent that was payed by the contract over its whole lifetime.
	///
	/// A restored contract starts with a value of zero just like a new contract.
	pub rent_payed: Balance,
	/// Last block rent has been payed.
	pub deduct_block: BlockNumber,
	/// Last block child storage has been written.
	pub last_write: Option<BlockNumber>,
	/// This field is reserved for future evolution of format.
	pub _reserved: Option<()>,
}

impl<CodeHash, Balance, BlockNumber> RawAliveContractInfo<CodeHash, Balance, BlockNumber> {
	/// Associated child trie unique id is built from the hash part of the trie id.
	pub fn child_trie_info(&self) -> ChildInfo {
		child_trie_info(&self.trie_id[..])
	}
}

/// Associated child trie unique id is built from the hash part of the trie id.
pub(crate) fn child_trie_info(trie_id: &[u8]) -> ChildInfo {
	ChildInfo::new_default(trie_id)
}

#[derive(Encode, Decode, PartialEq, Eq, RuntimeDebug)]
pub struct RawTombstoneContractInfo<H, Hasher>(H, PhantomData<Hasher>);

impl<H, Hasher> RawTombstoneContractInfo<H, Hasher>
where
	H: Member + MaybeSerializeDeserialize+ Debug
		+ AsRef<[u8]> + AsMut<[u8]> + Copy + Default
		+ sp_std::hash::Hash + Codec,
	Hasher: Hash<Output=H>,
{
	fn new(storage_root: &[u8], code_hash: H) -> Self {
		let mut buf = Vec::new();
		storage_root.using_encoded(|encoded| buf.extend_from_slice(encoded));
		buf.extend_from_slice(code_hash.as_ref());
		RawTombstoneContractInfo(<Hasher as Hash>::hash(&buf[..]), PhantomData)
	}
}

impl<T: Config> From<AliveContractInfo<T>> for ContractInfo<T> {
	fn from(alive_info: AliveContractInfo<T>) -> Self {
		Self::Alive(alive_info)
	}
}
