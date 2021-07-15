#[frame_support::pallet]
mod pallet {
	use frame_support::pallet_prelude::*;

	#[pallet::config]
	pub trait Config: frame_system::Config {}

	#[pallet::pallet]
	#[pallet::generate_store(trait Store)]
	pub struct Pallet<T>(core::marker::PhantomData<T>);

	#[pallet::storage]
	type Foo<T> = StorageValue<_, u8>;

	#[pallet::storage]
	#[pallet::storage_prefix = "Foo"]
	type NotFoo<T> = StorageValue<_, u16>;

	#[pallet::storage]
	type CounterForBar<T> = StorageValue<_, u16>;

	#[pallet::storage]
	type Bar<T> = CountedStorageMap<_, Twox64Concat, u16, u16>;
}

fn main() {
}
