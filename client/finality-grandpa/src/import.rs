// This file is part of Substrate.

// Copyright (C) 2018-2021 Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: GPL-3.0-or-later WITH Classpath-exception-2.0

// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with this program. If not, see <https://www.gnu.org/licenses/>.

use std::{collections::HashMap, marker::PhantomData, sync::Arc};

use log::debug;
use parity_scale_codec::Encode;

use sc_client_api::{backend::Backend, utils::is_descendent_of};
use sc_consensus::shared_data::{SharedDataLocked, SharedDataLockedUpgradable};
use sc_telemetry::TelemetryHandle;
use sp_api::TransactionFor;
use sp_blockchain::{well_known_cache_keys, BlockStatus};
use sp_consensus::{
	BlockCheckParams, BlockImport, BlockImportParams, BlockOrigin, Error as ConsensusError,
	ImportResult, JustificationImport, SelectChain,
};
use sp_finality_grandpa::{ConsensusLog, ScheduledChange, SetId, GRANDPA_ENGINE_ID};
use sp_runtime::generic::{BlockId, OpaqueDigestItemId};
use sp_runtime::traits::{Block as BlockT, DigestFor, Header as HeaderT, NumberFor, Zero};
use sp_runtime::Justification;
use sp_utils::mpsc::TracingUnboundedSender;

use crate::{
	authorities::{AuthoritySet, DelayKind, PendingChange, SharedAuthoritySet},
	environment::finalize_block,
	justification::GrandpaJustification,
	notification::GrandpaJustificationSender,
	ClientForGrandpa, CommandOrError, Error, NewAuthoritySet, VoterCommand,
};

/// A block-import handler for GRANDPA.
///
/// This scans each imported block for signals of changing authority set.
/// If the block being imported enacts an authority set change then:
/// - If the current authority set is still live: we import the block
/// - Otherwise, the block must include a valid justification.
///
/// When using GRANDPA, the block import worker should be using this block import
/// object.
pub struct GrandpaBlockImport<Backend, Block: BlockT, Client, SC> {
	inner: Arc<Client>,
	select_chain: SC,
	authority_set: SharedAuthoritySet<Block::Hash, NumberFor<Block>>,
	send_voter_commands: TracingUnboundedSender<VoterCommand<Block::Hash, NumberFor<Block>>>,
	authority_set_hard_forks: HashMap<Block::Hash, PendingChange<Block::Hash, NumberFor<Block>>>,
	justification_sender: GrandpaJustificationSender<Block>,
	telemetry: Option<TelemetryHandle>,
	_phantom: PhantomData<Backend>,
}

impl<Backend, Block: BlockT, Client, SC: Clone> Clone
	for GrandpaBlockImport<Backend, Block, Client, SC>
{
	fn clone(&self) -> Self {
		GrandpaBlockImport {
			inner: self.inner.clone(),
			select_chain: self.select_chain.clone(),
			authority_set: self.authority_set.clone(),
			send_voter_commands: self.send_voter_commands.clone(),
			authority_set_hard_forks: self.authority_set_hard_forks.clone(),
			justification_sender: self.justification_sender.clone(),
			telemetry: self.telemetry.clone(),
			_phantom: PhantomData,
		}
	}
}

#[async_trait::async_trait]
impl<BE, Block: BlockT, Client, SC> JustificationImport<Block>
	for GrandpaBlockImport<BE, Block, Client, SC>
where
	NumberFor<Block>: finality_grandpa::BlockNumberOps,
	DigestFor<Block>: Encode,
	BE: Backend<Block>,
	Client: ClientForGrandpa<Block, BE>,
	SC: SelectChain<Block>,
{
	type Error = ConsensusError;

	async fn on_start(&mut self) -> Vec<(Block::Hash, NumberFor<Block>)> {
		let mut out = Vec::new();
		let chain_info = self.inner.info();

		// request justifications for all pending changes for which change blocks have already been imported
		let pending_changes: Vec<_> = self
			.authority_set
			.inner()
			.pending_changes()
			.cloned()
			.collect();

		for pending_change in pending_changes {
			if pending_change.delay_kind == DelayKind::Finalized &&
				pending_change.effective_number() > chain_info.finalized_number &&
				pending_change.effective_number() <= chain_info.best_number
			{
				let effective_block_hash = if !pending_change.delay.is_zero() {
					self.select_chain
						.finality_target(
							pending_change.canon_hash,
							Some(pending_change.effective_number()),
						)
						.await
				} else {
					Ok(pending_change.canon_hash)
				};

				if let Ok(hash) = effective_block_hash {
					if let Ok(Some(header)) = self.inner.header(BlockId::Hash(hash)) {
						if *header.number() == pending_change.effective_number() {
							out.push((header.hash(), *header.number()));
						}
					}
				}
			}
		}

		out
	}

	async fn import_justification(
		&mut self,
		hash: Block::Hash,
		number: NumberFor<Block>,
		justification: Justification,
	) -> Result<(), Self::Error> {
		// this justification was requested by the sync service, therefore we
		// are not sure if it should enact a change or not. it could have been a
		// request made as part of initial sync but that means the justification
		// wasn't part of the block and was requested asynchronously, probably
		// makes sense to log in that case.
		GrandpaBlockImport::import_justification(self, hash, number, justification, false, false)
	}
}

enum AppliedChanges<H, N> {
	Standard(bool), // true if the change is ready to be applied (i.e. it's a root)
	Forced(NewAuthoritySet<H, N>),
	None,
}

impl<H, N> AppliedChanges<H, N> {
	fn needs_justification(&self) -> bool {
		match *self {
			AppliedChanges::Standard(_) => true,
			AppliedChanges::Forced(_) | AppliedChanges::None => false,
		}
	}
}

struct PendingSetChanges<Block: BlockT> {
	just_in_case: Option<(
		AuthoritySet<Block::Hash, NumberFor<Block>>,
		SharedDataLockedUpgradable<AuthoritySet<Block::Hash, NumberFor<Block>>>,
	)>,
	applied_changes: AppliedChanges<Block::Hash, NumberFor<Block>>,
	do_pause: bool,
}

impl<Block: BlockT> PendingSetChanges<Block> {
	// revert the pending set change explicitly.
	fn revert(self) {}

	fn defuse(mut self) -> (AppliedChanges<Block::Hash, NumberFor<Block>>, bool) {
		self.just_in_case = None;
		let applied_changes = std::mem::replace(&mut self.applied_changes, AppliedChanges::None);
		(applied_changes, self.do_pause)
	}
}

impl<Block: BlockT> Drop for PendingSetChanges<Block> {
	fn drop(&mut self) {
		if let Some((old_set, mut authorities)) = self.just_in_case.take() {
			*authorities.upgrade() = old_set;
		}
	}
}

/// Checks the given header for a consensus digest signalling a **standard** scheduled change and
/// extracts it.
pub fn find_scheduled_change<B: BlockT>(
	header: &B::Header,
) -> Option<ScheduledChange<NumberFor<B>>> {
	let id = OpaqueDigestItemId::Consensus(&GRANDPA_ENGINE_ID);

	let filter_log = |log: ConsensusLog<NumberFor<B>>| match log {
		ConsensusLog::ScheduledChange(change) => Some(change),
		_ => None,
	};

	// find the first consensus digest with the right ID which converts to
	// the right kind of consensus log.
	header.digest().convert_first(|l| l.try_to(id).and_then(filter_log))
}

/// Checks the given header for a consensus digest signalling a **forced** scheduled change and
/// extracts it.
pub fn find_forced_change<B: BlockT>(
	header: &B::Header,
) -> Option<(NumberFor<B>, ScheduledChange<NumberFor<B>>)> {
	let id = OpaqueDigestItemId::Consensus(&GRANDPA_ENGINE_ID);

	let filter_log = |log: ConsensusLog<NumberFor<B>>| match log {
		ConsensusLog::ForcedChange(delay, change) => Some((delay, change)),
		_ => None,
	};

	// find the first consensus digest with the right ID which converts to
	// the right kind of consensus log.
	header.digest().convert_first(|l| l.try_to(id).and_then(filter_log))
}

impl<BE, Block: BlockT, Client, SC> GrandpaBlockImport<BE, Block, Client, SC>
where
	NumberFor<Block>: finality_grandpa::BlockNumberOps,
	DigestFor<Block>: Encode,
	BE: Backend<Block>,
	Client: ClientForGrandpa<Block, BE>,
{
	// check for a new authority set change.
	fn check_new_change(
		&self,
		header: &Block::Header,
		hash: Block::Hash,
	) -> Option<PendingChange<Block::Hash, NumberFor<Block>>> {
		// check for forced authority set hard forks
		if let Some(change) = self.authority_set_hard_forks.get(&hash) {
			return Some(change.clone());
		}

		// check for forced change.
		if let Some((median_last_finalized, change)) = find_forced_change::<Block>(header) {
			return Some(PendingChange {
				next_authorities: change.next_authorities,
				delay: change.delay,
				canon_height: *header.number(),
				canon_hash: hash,
				delay_kind: DelayKind::Best { median_last_finalized },
			});
		}

		// check normal scheduled change.
		let change = find_scheduled_change::<Block>(header)?;
		Some(PendingChange {
			next_authorities: change.next_authorities,
			delay: change.delay,
			canon_height: *header.number(),
			canon_hash: hash,
			delay_kind: DelayKind::Finalized,
		})
	}

	fn make_authorities_changes(
		&self,
		block: &mut BlockImportParams<Block, TransactionFor<Client, Block>>,
		hash: Block::Hash,
		initial_sync: bool,
	) -> Result<PendingSetChanges<Block>, ConsensusError> {
		// when we update the authorities, we need to hold the lock
		// until the block is written to prevent a race if we need to restore
		// the old authority set on error or panic.
		struct InnerGuard<'a, H, N> {
			old: Option<AuthoritySet<H, N>>,
			guard: Option<SharedDataLocked<'a, AuthoritySet<H, N>>>,
		}

		impl<'a, H, N> InnerGuard<'a, H, N> {
			fn as_mut(&mut self) -> &mut AuthoritySet<H, N> {
				&mut **self.guard.as_mut().expect("only taken on deconstruction; qed")
			}

			fn set_old(&mut self, old: AuthoritySet<H, N>) {
				if self.old.is_none() {
					// ignore "newer" old changes.
					self.old = Some(old);
				}
			}

			fn consume(
				mut self,
			) -> Option<(AuthoritySet<H, N>, SharedDataLocked<'a, AuthoritySet<H, N>>)> {
				self.old.take().map(|old| (
						old,
						self.guard.take().expect("only taken on deconstruction; qed"),
				))
			}
		}

		impl<'a, H, N> Drop for InnerGuard<'a, H, N> {
			fn drop(&mut self) {
				if let (Some(mut guard), Some(old)) = (self.guard.take(), self.old.take()) {
					*guard = old;
				}
			}
		}

		let number = *(block.header.number());
		let maybe_change = self.check_new_change(
			&block.header,
			hash,
		);

		// returns a function for checking whether a block is a descendent of another
		// consistent with querying client directly after importing the block.
		let parent_hash = *block.header.parent_hash();
		let is_descendent_of = is_descendent_of(&*self.inner, Some((hash, parent_hash)));

		let mut guard = InnerGuard {
			guard: Some(self.authority_set.inner_locked()),
			old: None,
		};

		// whether to pause the old authority set -- happens after import
		// of a forced change block.
		let mut do_pause = false;

		// add any pending changes.
		if let Some(change) = maybe_change {
			let old = guard.as_mut().clone();
			guard.set_old(old);

			if let DelayKind::Best { .. } = change.delay_kind {
				do_pause = true;
			}

			guard.as_mut().add_pending_change(
				change,
				&is_descendent_of,
			).map_err(|e| ConsensusError::ClientImport(e.to_string()))?;
		}

		let applied_changes = {
			let forced_change_set = guard
				.as_mut()
				.apply_forced_changes(
					hash,
					number,
					&is_descendent_of,
					initial_sync,
					self.telemetry.clone(),
				)
				.map_err(|e| ConsensusError::ClientImport(e.to_string()))
				.map_err(ConsensusError::from)?;

			if let Some((median_last_finalized_number, new_set)) = forced_change_set {
				let new_authorities = {
					let (set_id, new_authorities) = new_set.current();

					// we will use the median last finalized number as a hint
					// for the canon block the new authority set should start
					// with. we use the minimum between the median and the local
					// best finalized block.
					let best_finalized_number = self.inner.info().finalized_number;
					let canon_number = best_finalized_number.min(median_last_finalized_number);
					let canon_hash =
						self.inner.header(BlockId::Number(canon_number))
							.map_err(|e| ConsensusError::ClientImport(e.to_string()))?
							.expect(
								"the given block number is less or equal than the current best finalized number; \
								 current best finalized number must exist in chain; qed."
							)
							.hash();

					NewAuthoritySet {
						canon_number,
						canon_hash,
						set_id,
						authorities: new_authorities.to_vec(),
					}
				};
				let old = ::std::mem::replace(guard.as_mut(), new_set);
				guard.set_old(old);

				AppliedChanges::Forced(new_authorities)
			} else {
				let did_standard = guard.as_mut().enacts_standard_change(hash, number, &is_descendent_of)
					.map_err(|e| ConsensusError::ClientImport(e.to_string()))
					.map_err(ConsensusError::from)?;

				if let Some(root) = did_standard {
					AppliedChanges::Standard(root)
				} else {
					AppliedChanges::None
				}
			}
		};

		// consume the guard safely and write necessary changes.
		let just_in_case = guard.consume();
		if let Some((_, ref authorities)) = just_in_case {
			let authorities_change = match applied_changes {
				AppliedChanges::Forced(ref new) => Some(new),
				AppliedChanges::Standard(_) => None, // the change isn't actually applied yet.
				AppliedChanges::None => None,
			};

			crate::aux_schema::update_authority_set::<Block, _, _>(
				authorities,
				authorities_change,
				|insert| block.auxiliary.extend(
					insert.iter().map(|(k, v)| (k.to_vec(), Some(v.to_vec())))
				)
			);
		}

		let just_in_case = just_in_case.map(|(o, i)| (o, i.release_mutex()));

		Ok(PendingSetChanges {
			just_in_case,
			applied_changes,
			do_pause,
		})
	}
}

#[async_trait::async_trait]
impl<BE, Block: BlockT, Client, SC> BlockImport<Block> for GrandpaBlockImport<BE, Block, Client, SC>
where
	NumberFor<Block>: finality_grandpa::BlockNumberOps,
	DigestFor<Block>: Encode,
	BE: Backend<Block>,
	Client: ClientForGrandpa<Block, BE>,
	for<'a> &'a Client:
		BlockImport<Block, Error = ConsensusError, Transaction = TransactionFor<Client, Block>>,
	TransactionFor<Client, Block>: 'static,
	SC: Send,
{
	type Error = ConsensusError;
	type Transaction = TransactionFor<Client, Block>;

	async fn import_block(
		&mut self,
		mut block: BlockImportParams<Block, Self::Transaction>,
		new_cache: HashMap<well_known_cache_keys::Id, Vec<u8>>,
	) -> Result<ImportResult, Self::Error> {
		let hash = block.post_hash();
		let number = *block.header.number();

		// early exit if block already in chain, otherwise the check for
		// authority changes will error when trying to re-import a change block
		match self.inner.status(BlockId::Hash(hash)) {
			Ok(BlockStatus::InChain) => {
				// Strip justifications when re-importing an existing block.
				let _justifications = block.justifications.take();
				return (&*self.inner).import_block(block, new_cache).await
			}
			Ok(BlockStatus::Unknown) => {},
			Err(e) => return Err(ConsensusError::ClientImport(e.to_string())),
		}

		// on initial sync we will restrict logging under info to avoid spam.
		let initial_sync = block.origin == BlockOrigin::NetworkInitialSync;

		let pending_changes = self.make_authorities_changes(&mut block, hash, initial_sync)?;

		// we don't want to finalize on `inner.import_block`
		let mut justifications = block.justifications.take();
		let import_result = (&*self.inner).import_block(block, new_cache).await;

		let mut imported_aux = {
			match import_result {
				Ok(ImportResult::Imported(aux)) => aux,
				Ok(r) => {
					debug!(
						target: "afg",
						"Restoring old authority set after block import result: {:?}",
						r,
					);
					pending_changes.revert();
					return Ok(r);
				},
				Err(e) => {
					debug!(
						target: "afg",
						"Restoring old authority set after block import error: {:?}",
						e,
					);
					pending_changes.revert();
					return Err(ConsensusError::ClientImport(e.to_string()));
				},
			}
		};

		let (applied_changes, do_pause) = pending_changes.defuse();

		// Send the pause signal after import but BEFORE sending a `ChangeAuthorities` message.
		if do_pause {
			let _ = self.send_voter_commands.unbounded_send(
				VoterCommand::Pause("Forced change scheduled after inactivity".to_string())
			);
		}

		let needs_justification = applied_changes.needs_justification();

		match applied_changes {
			AppliedChanges::Forced(new) => {
				// NOTE: when we do a force change we are "discrediting" the old set so we
				// ignore any justifications from them. this block may contain a justification
				// which should be checked and imported below against the new authority
				// triggered by this forced change. the new grandpa voter will start at the
				// last median finalized block (which is before the block that enacts the
				// change), full nodes syncing the chain will not be able to successfully
				// import justifications for those blocks since their local authority set view
				// is still of the set before the forced change was enacted, still after #1867
				// they should import the block and discard the justification, and they will
				// then request a justification from sync if it's necessary (which they should
				// then be able to successfully validate).
				let _ = self.send_voter_commands.unbounded_send(VoterCommand::ChangeAuthorities(new));

				// we must clear all pending justifications requests, presumably they won't be
				// finalized hence why this forced changes was triggered
				imported_aux.clear_justification_requests = true;
			},
			AppliedChanges::Standard(false) => {
				// we can't apply this change yet since there are other dependent changes that we
				// need to apply first, drop any justification that might have been provided with
				// the block to make sure we request them from `sync` which will ensure they'll be
				// applied in-order.
				justifications.take();
			},
			_ => {},
		}

		let grandpa_justification = justifications
			.and_then(|just| just.into_justification(GRANDPA_ENGINE_ID));

		match grandpa_justification {
			Some(justification) => {
				let import_res = self.import_justification(
					hash,
					number,
					(GRANDPA_ENGINE_ID, justification),
					needs_justification,
					initial_sync,
				);

				import_res.unwrap_or_else(|err| {
					if needs_justification {
						debug!(target: "afg", "Imported block #{} that enacts authority set change with \
							invalid justification: {:?}, requesting justification from peers.", number, err);
						imported_aux.bad_justification = true;
						imported_aux.needs_justification = true;
					}
				});
			},
			None => {
				if needs_justification {
					debug!(
						target: "afg",
						"Imported unjustified block #{} that enacts authority set change, waiting for finality for enactment.",
						number,
					);

					imported_aux.needs_justification = true;
				}
			}
		}

		Ok(ImportResult::Imported(imported_aux))
	}

	async fn check_block(
		&mut self,
		block: BlockCheckParams<Block>,
	) -> Result<ImportResult, Self::Error> {
		self.inner.check_block(block).await
	}
}

impl<Backend, Block: BlockT, Client, SC> GrandpaBlockImport<Backend, Block, Client, SC> {
	pub(crate) fn new(
		inner: Arc<Client>,
		select_chain: SC,
		authority_set: SharedAuthoritySet<Block::Hash, NumberFor<Block>>,
		send_voter_commands: TracingUnboundedSender<VoterCommand<Block::Hash, NumberFor<Block>>>,
		authority_set_hard_forks: Vec<(SetId, PendingChange<Block::Hash, NumberFor<Block>>)>,
		justification_sender: GrandpaJustificationSender<Block>,
		telemetry: Option<TelemetryHandle>,
	) -> GrandpaBlockImport<Backend, Block, Client, SC> {
		// check for and apply any forced authority set hard fork that applies
		// to the *current* authority set.
		if let Some((_, change)) = authority_set_hard_forks
			.iter()
			.find(|(set_id, _)| *set_id == authority_set.set_id())
		{
			authority_set.inner().current_authorities = change.next_authorities.clone();
		}

		// index authority set hard forks by block hash so that they can be used
		// by any node syncing the chain and importing a block hard fork
		// authority set changes.
		let authority_set_hard_forks = authority_set_hard_forks
			.into_iter()
			.map(|(_, change)| (change.canon_hash, change))
			.collect::<HashMap<_, _>>();

		// check for and apply any forced authority set hard fork that apply to
		// any *pending* standard changes, checking by the block hash at which
		// they were announced.
		{
			let mut authority_set = authority_set.inner();

			authority_set.pending_standard_changes = authority_set
				.pending_standard_changes
				.clone()
				.map(&mut |hash, _, original| {
					authority_set_hard_forks
						.get(&hash)
						.cloned()
						.unwrap_or(original)
				});
		}

		GrandpaBlockImport {
			inner,
			select_chain,
			authority_set,
			send_voter_commands,
			authority_set_hard_forks,
			justification_sender,
			telemetry,
			_phantom: PhantomData,
		}
	}
}

impl<BE, Block: BlockT, Client, SC> GrandpaBlockImport<BE, Block, Client, SC>
where
	BE: Backend<Block>,
	Client: ClientForGrandpa<Block, BE>,
	NumberFor<Block>: finality_grandpa::BlockNumberOps,
{
	/// Import a block justification and finalize the block.
	///
	/// If `enacts_change` is set to true, then finalizing this block *must*
	/// enact an authority set change, the function will panic otherwise.
	fn import_justification(
		&mut self,
		hash: Block::Hash,
		number: NumberFor<Block>,
		justification: Justification,
		enacts_change: bool,
		initial_sync: bool,
	) -> Result<(), ConsensusError> {
		if justification.0 != GRANDPA_ENGINE_ID {
			// TODO: the import queue needs to be refactored to be able dispatch to the correct
			// `JustificationImport` instance based on `ConsensusEngineId`, or we need to build a
			// justification import pipeline similar to what we do for `BlockImport`. In the
			// meantime we'll just drop the justification, since this is only used for BEEFY which
			// is still WIP.
			return Ok(());
		}

		let justification = GrandpaJustification::decode_and_verify_finalizes(
			&justification.1,
			(hash, number),
			self.authority_set.set_id(),
			&self.authority_set.current_authorities(),
		);

		let justification = match justification {
			Err(e) => return Err(ConsensusError::ClientImport(e.to_string())),
			Ok(justification) => justification,
		};

		let result = finalize_block(
			self.inner.clone(),
			&self.authority_set,
			None,
			hash,
			number,
			justification.into(),
			initial_sync,
			Some(&self.justification_sender),
			self.telemetry.clone(),
		);

		match result {
			Err(CommandOrError::VoterCommand(command)) => {
				afg_log!(initial_sync,
					"👴 Imported justification for block #{} that triggers \
					command {}, signaling voter.",
					number,
					command,
				);

				// send the command to the voter
				let _ = self.send_voter_commands.unbounded_send(command);
			},
			Err(CommandOrError::Error(e)) => {
				return Err(match e {
					Error::Grandpa(error) => ConsensusError::ClientImport(error.to_string()),
					Error::Network(error) => ConsensusError::ClientImport(error),
					Error::Blockchain(error) => ConsensusError::ClientImport(error),
					Error::Client(error) => ConsensusError::ClientImport(error.to_string()),
					Error::Safety(error) => ConsensusError::ClientImport(error),
					Error::Signing(error) => ConsensusError::ClientImport(error),
					Error::Timer(error) => ConsensusError::ClientImport(error.to_string()),
					Error::RuntimeApi(error) => ConsensusError::ClientImport(error.to_string()),
				});
			},
			Ok(_) => {
				assert!(!enacts_change, "returns Ok when no authority set change should be enacted; qed;");
			},
		}

		Ok(())
	}
}
