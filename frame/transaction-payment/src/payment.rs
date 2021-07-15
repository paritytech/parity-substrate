///! Traits and default implementation for paying transaction fees.

use crate::Config;

use codec::FullCodec;
use sp_runtime::{
	traits::{AtLeast32BitUnsigned, DispatchInfoOf, MaybeSerializeDeserialize, PostDispatchInfoOf, Saturating, Zero},
	transaction_validity::InvalidTransaction,
};
use sp_std::{fmt::Debug, marker::PhantomData};

use frame_support::{
	traits::{Currency, ExistenceRequirement, Get, Imbalance, OnUnbalanced, WithdrawReasons},
	unsigned::TransactionValidityError,
};

type NegativeImbalanceOf<C, T> =
	<C as Currency<<T as frame_system::Config>::AccountId>>::NegativeImbalance;

/// Handle withdrawing, refunding and depositing of transaction fees.
pub trait OnChargeTransaction<T: Config> {
	/// The underlying integer type in which fees are calculated.
	type Balance: AtLeast32BitUnsigned + FullCodec + Copy + MaybeSerializeDeserialize + Debug + Default
		+ scale_info::TypeInfo;
	type LiquidityInfo: Default;

	/// Before the transaction is executed the payment of the transaction fees
	/// need to be secured.
	///
	/// Note: The `fee` already includes the `tip`.
	fn withdraw_fee(
		who: &T::AccountId,
		call: &T::Call,
		dispatch_info: &DispatchInfoOf<T::Call>,
		fee: Self::Balance,
		tip: Self::Balance,
	) -> Result<Self::LiquidityInfo, TransactionValidityError>;

	/// After the transaction was executed the actual fee can be calculated.
	/// This function should refund any overpaid fees and optionally deposit
	/// the corrected amount.
	///
	/// Note: The `fee` already includes the `tip`.
	fn correct_and_deposit_fee(
		who: &T::AccountId,
		dispatch_info: &DispatchInfoOf<T::Call>,
		post_info: &PostDispatchInfoOf<T::Call>,
		corrected_fee: Self::Balance,
		tip: Self::Balance,
		already_withdrawn: Self::LiquidityInfo,
	) -> Result<(), TransactionValidityError>;
}

/// Implements the transaction payment for a pallet implementing the `Currency`
/// trait (eg. the pallet_balances) using an unbalance handler (implementing
/// `OnUnbalanced`).
///
/// The unbalance handler is given 2 unbalanceds in [`OnUnbalanced::on_unbalanceds`]: fee and
/// then tip.
pub struct CurrencyAdapter<C, OU>(PhantomData<(C, OU)>);

/// Default implementation for a Currency and an OnUnbalanced handler.
///
/// The unbalance handler is given 2 unbalanceds in [`OnUnbalanced::on_unbalanceds`]: fee and
/// then tip.
impl<T, C, OU> OnChargeTransaction<T> for CurrencyAdapter<C, OU>
where
	T: Config,
	T::TransactionByteFee: Get<<C as Currency<<T as frame_system::Config>::AccountId>>::Balance>,
	C: Currency<<T as frame_system::Config>::AccountId>,
	C::PositiveImbalance:
		Imbalance<<C as Currency<<T as frame_system::Config>::AccountId>>::Balance, Opposite = C::NegativeImbalance>,
	C::NegativeImbalance:
		Imbalance<<C as Currency<<T as frame_system::Config>::AccountId>>::Balance, Opposite = C::PositiveImbalance>,
	OU: OnUnbalanced<NegativeImbalanceOf<C, T>>,
{
	type LiquidityInfo = Option<NegativeImbalanceOf<C, T>>;
	type Balance = <C as Currency<<T as frame_system::Config>::AccountId>>::Balance;

	/// Withdraw the predicted fee from the transaction origin.
	///
	/// Note: The `fee` already includes the `tip`.
	fn withdraw_fee(
		who: &T::AccountId,
		_call: &T::Call,
		_info: &DispatchInfoOf<T::Call>,
		fee: Self::Balance,
		tip: Self::Balance,
	) -> Result<Self::LiquidityInfo, TransactionValidityError> {
		if fee.is_zero() {
			return Ok(None);
		}

		let withdraw_reason = if tip.is_zero() {
			WithdrawReasons::TRANSACTION_PAYMENT
		} else {
			WithdrawReasons::TRANSACTION_PAYMENT | WithdrawReasons::TIP
		};

		match C::withdraw(who, fee, withdraw_reason, ExistenceRequirement::KeepAlive) {
			Ok(imbalance) => Ok(Some(imbalance)),
			Err(_) => Err(InvalidTransaction::Payment.into()),
		}
	}

	/// Hand the fee and the tip over to the `[OnUnbalanced]` implementation.
	/// Since the predicted fee might have been too high, parts of the fee may
	/// be refunded.
	///
	/// Note: The `corrected_fee` already includes the `tip`.
	fn correct_and_deposit_fee(
		who: &T::AccountId,
		_dispatch_info: &DispatchInfoOf<T::Call>,
		_post_info: &PostDispatchInfoOf<T::Call>,
		corrected_fee: Self::Balance,
		tip: Self::Balance,
		already_withdrawn: Self::LiquidityInfo,
	) -> Result<(), TransactionValidityError> {
		if let Some(paid) = already_withdrawn {
			// Calculate how much refund we should return
			let refund_amount = paid.peek().saturating_sub(corrected_fee);
			// refund to the the account that paid the fees. If this fails, the
			// account might have dropped below the existential balance. In
			// that case we don't refund anything.
			let refund_imbalance =
				C::deposit_into_existing(&who, refund_amount).unwrap_or_else(|_| C::PositiveImbalance::zero());
			// merge the imbalance caused by paying the fees and refunding parts of it again.
			let adjusted_paid = paid
				.offset(refund_imbalance)
				.same()
				.map_err(|_| TransactionValidityError::Invalid(InvalidTransaction::Payment))?;
			// Call someone else to handle the imbalance (fee and tip separately)
			let (tip, fee) = adjusted_paid.split(tip);
			OU::on_unbalanceds(Some(fee).into_iter().chain(Some(tip)));
		}
		Ok(())
	}
}
