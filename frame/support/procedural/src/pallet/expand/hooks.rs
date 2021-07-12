// This file is part of Substrate.

// Copyright (C) 2020-2021 Parity Technologies (UK) Ltd.
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

use crate::pallet::Def;

/// * implement the individual traits using the Hooks trait
pub fn expand_hooks(def: &mut Def) -> proc_macro2::TokenStream {
	let (where_clause, span, has_runtime_upgrade) = match def.hooks.as_ref() {
		Some(hooks) => {
			let where_clause = hooks.where_clause.clone();
			let span = hooks.attr_span;
			let has_runtime_upgrade = hooks.has_runtime_upgrade;
			(where_clause, span, has_runtime_upgrade)
		},
		None => (None, def.pallet_struct.attr_span, false),
	};

	let frame_support = &def.frame_support;
	let type_impl_gen = &def.type_impl_generics(span);
	let type_use_gen = &def.type_use_generics(span);
	let pallet_ident = &def.pallet_struct.pallet;
	let frame_system = &def.frame_system;

	let log_runtime_upgrade = if has_runtime_upgrade {
		// a migration is defined here.
		quote::quote! {
			#frame_support::log::info!(
				target: #frame_support::LOG_TARGET,
				"⚠️ {} declares internal migrations (which *might* execute)",
				pallet_name,
			);
		}
	} else {
		// default.
		quote::quote! {
			#frame_support::log::info!(
				target: #frame_support::LOG_TARGET,
				"✅ no migration for {}",
				pallet_name,
			);
		}
	};

	let hooks_impl = if def.hooks.is_none() {
		let frame_system = &def.frame_system;
		quote::quote!{
			impl<#type_impl_gen>
				#frame_support::traits::Hooks<<T as #frame_system::Config>::BlockNumber>
				for Pallet<#type_use_gen> {}
		}
	} else {
		proc_macro2::TokenStream::new()
	};

	quote::quote_spanned!(span =>
		#hooks_impl

		impl<#type_impl_gen>
			#frame_support::traits::OnFinalize<<T as #frame_system::Config>::BlockNumber>
			for #pallet_ident<#type_use_gen> #where_clause
		{
			fn on_finalize(n: <T as #frame_system::Config>::BlockNumber) {
				#frame_support::sp_tracing::enter_span!(
					#frame_support::sp_tracing::trace_span!("on_finalize")
				);
				<
					Self as #frame_support::traits::Hooks<
						<T as #frame_system::Config>::BlockNumber
					>
				>::on_finalize(n)
			}
		}

		impl<#type_impl_gen>
			#frame_support::traits::OnIdle<<T as #frame_system::Config>::BlockNumber>
			for #pallet_ident<#type_use_gen> #where_clause
		{
			fn on_idle(
				n: <T as #frame_system::Config>::BlockNumber,
				remaining_weight: #frame_support::weights::Weight
			) -> #frame_support::weights::Weight {
				<
					Self as #frame_support::traits::Hooks<
						<T as #frame_system::Config>::BlockNumber
					>
				>::on_idle(n, remaining_weight)
			}
		}

		impl<#type_impl_gen>
			#frame_support::traits::OnInitialize<<T as #frame_system::Config>::BlockNumber>
			for #pallet_ident<#type_use_gen> #where_clause
		{
			fn on_initialize(
				n: <T as #frame_system::Config>::BlockNumber
			) -> #frame_support::weights::Weight {
				#frame_support::sp_tracing::enter_span!(
					#frame_support::sp_tracing::trace_span!("on_initialize")
				);
				<
					Self as #frame_support::traits::Hooks<
						<T as #frame_system::Config>::BlockNumber
					>
				>::on_initialize(n)
			}
		}

		impl<#type_impl_gen>
			#frame_support::traits::OnRuntimeUpgrade
			for #pallet_ident<#type_use_gen> #where_clause
		{
			fn on_runtime_upgrade() -> #frame_support::weights::Weight {
				#frame_support::sp_tracing::enter_span!(
					#frame_support::sp_tracing::trace_span!("on_runtime_update")
				);

				// log info about the upgrade.
				let pallet_name = <
					<T as #frame_system::Config>::PalletInfo
					as
					#frame_support::traits::PalletInfo
				>::name::<Self>().unwrap_or("<unknown pallet name>");
				#log_runtime_upgrade

				<
					Self as #frame_support::traits::Hooks<
						<T as #frame_system::Config>::BlockNumber
					>
				>::on_runtime_upgrade()
			}

			#[cfg(feature = "try-runtime")]
			fn pre_upgrade() -> Result<(), &'static str> {
				<
					Self
					as
					#frame_support::traits::Hooks<<T as #frame_system::Config>::BlockNumber>
				>::pre_upgrade()
			}

			#[cfg(feature = "try-runtime")]
			fn post_upgrade() -> Result<(), &'static str> {
				<
					Self
					as
					#frame_support::traits::Hooks<<T as #frame_system::Config>::BlockNumber>
				>::post_upgrade()
			}
		}

		impl<#type_impl_gen>
			#frame_support::traits::OffchainWorker<<T as #frame_system::Config>::BlockNumber>
			for #pallet_ident<#type_use_gen> #where_clause
		{
			fn offchain_worker(n: <T as #frame_system::Config>::BlockNumber) {
				<
					Self as #frame_support::traits::Hooks<
						<T as #frame_system::Config>::BlockNumber
					>
				>::offchain_worker(n)
			}
		}

		impl<#type_impl_gen>
			#frame_support::traits::IntegrityTest
			for #pallet_ident<#type_use_gen> #where_clause
		{
			fn integrity_test() {
				<
					Self as #frame_support::traits::Hooks<
						<T as #frame_system::Config>::BlockNumber
					>
				>::integrity_test()
			}
		}
	)
}
