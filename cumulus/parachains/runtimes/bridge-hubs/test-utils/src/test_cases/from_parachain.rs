// Copyright (C) Parity Technologies (UK) Ltd.
// This file is part of Cumulus.

// Cumulus is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Cumulus is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Cumulus.  If not, see <http://www.gnu.org/licenses/>.

//! Module contains predefined test-case scenarios for `Runtime` with bridging capabilities
//! with remote parachain.

use crate::{
	test_cases::{bridges_prelude::*, helpers, run_test},
	test_data,
};

use bp_header_chain::ChainWithGrandpa;
use bp_messages::{
	source_chain::TargetHeaderChain, target_chain::SourceHeaderChain, LaneId,
	UnrewardedRelayersState,
};
use bp_polkadot_core::parachains::ParaHash;
use bp_relayers::{RewardsAccountOwner, RewardsAccountParams};
use bp_runtime::{HashOf, Parachain, UnderlyingChainOf};
use bridge_runtime_common::{
	messages::{
		source::FromBridgedChainMessagesDeliveryProof, target::FromBridgedChainMessagesProof,
		BridgedChain as MessageBridgedChain, MessageBridge, ThisChain as MessageThisChain,
	},
	messages_xcm_extension::XcmAsPlainPayload,
};
use frame_support::traits::{Get, OnFinalize, OnInitialize};
use frame_system::pallet_prelude::BlockNumberFor;
use parachains_runtimes_test_utils::{
	AccountIdOf, BasicParachainRuntime, CollatorSessionKeys, RuntimeCallOf,
};
use sp_keyring::AccountKeyring::*;
use sp_runtime::{traits::Header as HeaderT, AccountId32};
use xcm::latest::prelude::*;

/// Helper trait to test bridges with remote parachain.
///
/// This is only used to decrease amount of lines, dedicated to bounds.
pub trait WithRemoteParachainHelper {
	/// This chain runtime.
	type Runtime: BasicParachainRuntime
		+ cumulus_pallet_xcmp_queue::Config
		+ BridgeGrandpaConfig<Self::GPI>
		+ BridgeParachainsConfig<Self::PPI>
		+ BridgeMessagesConfig<
			Self::MPI,
			InboundPayload = XcmAsPlainPayload,
			InboundRelayer = bp_runtime::AccountIdOf<MessageBridgedChain<Self::MB>>,
			OutboundPayload = XcmAsPlainPayload,
		> + pallet_bridge_relayers::Config;
	/// All pallets of this chain, excluding system pallet.
	type AllPalletsWithoutSystem: OnInitialize<BlockNumberFor<Self::Runtime>>
		+ OnFinalize<BlockNumberFor<Self::Runtime>>;
	/// Instance of the `pallet-bridge-grandpa`, used to bridge with remote relay chain.
	type GPI: 'static;
	/// Instance of the `pallet-bridge-parachains`, used to bridge with remote parachain.
	type PPI: 'static;
	/// Instance of the `pallet-bridge-messages`, used to bridge with remote parachain.
	type MPI: 'static;
	/// Messages bridge definition.
	type MB: MessageBridge;
}

/// Adapter struct that implements `WithRemoteParachainHelper`.
pub struct WithRemoteParachainHelperAdapter<Runtime, AllPalletsWithoutSystem, GPI, PPI, MPI, MB>(
	sp_std::marker::PhantomData<(Runtime, AllPalletsWithoutSystem, GPI, PPI, MPI, MB)>,
);

impl<Runtime, AllPalletsWithoutSystem, GPI, PPI, MPI, MB> WithRemoteParachainHelper
	for WithRemoteParachainHelperAdapter<Runtime, AllPalletsWithoutSystem, GPI, PPI, MPI, MB>
where
	Runtime: BasicParachainRuntime
		+ cumulus_pallet_xcmp_queue::Config
		+ BridgeGrandpaConfig<GPI>
		+ BridgeParachainsConfig<PPI>
		+ BridgeMessagesConfig<
			MPI,
			InboundPayload = XcmAsPlainPayload,
			InboundRelayer = bp_runtime::AccountIdOf<MessageBridgedChain<MB>>,
			OutboundPayload = XcmAsPlainPayload,
		> + pallet_bridge_relayers::Config,
	AllPalletsWithoutSystem:
		OnInitialize<BlockNumberFor<Runtime>> + OnFinalize<BlockNumberFor<Runtime>>,
	GPI: 'static,
	PPI: 'static,
	MPI: 'static,
	MB: MessageBridge,
{
	type Runtime = Runtime;
	type AllPalletsWithoutSystem = AllPalletsWithoutSystem;
	type GPI = GPI;
	type PPI = PPI;
	type MPI = MPI;
	type MB = MB;
}

/// Test-case makes sure that Runtime can dispatch XCM messages submitted by relayer,
/// with proofs (finality, para heads, message) independently submitted.
/// Also verifies relayer transaction signed extensions work as intended.
pub fn relayed_incoming_message_works<RuntimeHelper>(
	collator_session_key: CollatorSessionKeys<RuntimeHelper::Runtime>,
	runtime_para_id: u32,
	bridged_para_id: u32,
	bridged_chain_id: bp_runtime::ChainId,
	sibling_parachain_id: u32,
	local_relay_chain_id: NetworkId,
	lane_id: LaneId,
	prepare_configuration: impl Fn(),
	construct_and_apply_extrinsic: fn(
		sp_keyring::AccountKeyring,
		<RuntimeHelper::Runtime as frame_system::Config>::RuntimeCall,
	) -> sp_runtime::DispatchOutcome,
) where
	RuntimeHelper: WithRemoteParachainHelper,
	AccountIdOf<RuntimeHelper::Runtime>: From<AccountId32>,
	RuntimeCallOf<RuntimeHelper::Runtime>: From<BridgeGrandpaCall<RuntimeHelper::Runtime, RuntimeHelper::GPI>>
		+ From<BridgeParachainsCall<RuntimeHelper::Runtime, RuntimeHelper::PPI>>
		+ From<BridgeMessagesCall<RuntimeHelper::Runtime, RuntimeHelper::MPI>>,
	UnderlyingChainOf<MessageBridgedChain<RuntimeHelper::MB>>:
		bp_runtime::Chain<Hash = ParaHash> + Parachain,
	<RuntimeHelper::Runtime as BridgeGrandpaConfig<RuntimeHelper::GPI>>::BridgedChain:
		bp_runtime::Chain<Hash = RelayBlockHash, BlockNumber = RelayBlockNumber> + ChainWithGrandpa,
	<RuntimeHelper::Runtime as BridgeMessagesConfig<RuntimeHelper::MPI>>::SourceHeaderChain:
		SourceHeaderChain<
			MessagesProof = FromBridgedChainMessagesProof<
				HashOf<MessageBridgedChain<RuntimeHelper::MB>>,
			>,
		>,
{
	helpers::relayed_incoming_message_works::<
		RuntimeHelper::Runtime,
		RuntimeHelper::AllPalletsWithoutSystem,
		RuntimeHelper::MPI,
	>(
		collator_session_key,
		runtime_para_id,
		sibling_parachain_id,
		local_relay_chain_id,
		construct_and_apply_extrinsic,
		|relayer_id_at_this_chain,
		 relayer_id_at_bridged_chain,
		 message_destination,
		 message_nonce,
		 xcm| {
			let para_header_number = 5;
			let relay_header_number = 1;

			prepare_configuration();

			// start with bridged relay chain block#0
			helpers::initialize_bridge_grandpa_pallet::<RuntimeHelper::Runtime, RuntimeHelper::GPI>(
				test_data::initialization_data::<RuntimeHelper::Runtime, RuntimeHelper::GPI>(0),
			);

			// generate bridged relay chain finality, parachain heads and message proofs,
			// to be submitted by relayer to this chain.
			let (
				relay_chain_header,
				grandpa_justification,
				parachain_head,
				parachain_heads,
				para_heads_proof,
				message_proof,
			) = test_data::from_parachain::make_complex_relayer_delivery_proofs::<
				<RuntimeHelper::Runtime as BridgeGrandpaConfig<RuntimeHelper::GPI>>::BridgedChain,
				RuntimeHelper::MB,
				(),
			>(
				lane_id,
				xcm.into(),
				message_nonce,
				message_destination,
				para_header_number,
				relay_header_number,
				bridged_para_id,
			);

			let parachain_head_hash = parachain_head.hash();
			let relay_chain_header_hash = relay_chain_header.hash();
			let relay_chain_header_number = *relay_chain_header.number();
			vec![
				(
					BridgeGrandpaCall::<RuntimeHelper::Runtime, RuntimeHelper::GPI>::submit_finality_proof {
						finality_target: Box::new(relay_chain_header),
						justification: grandpa_justification,
					}.into(),
					helpers::VerifySubmitGrandpaFinalityProofOutcome::<RuntimeHelper::Runtime, RuntimeHelper::GPI>::expect_best_header_hash(
						relay_chain_header_hash,
					),
				),
				(
					BridgeParachainsCall::<RuntimeHelper::Runtime, RuntimeHelper::PPI>::submit_parachain_heads {
						at_relay_block: (relay_chain_header_number, relay_chain_header_hash),
						parachains: parachain_heads,
						parachain_heads_proof: para_heads_proof,
					}.into(),
					helpers::VerifySubmitParachainHeaderProofOutcome::<RuntimeHelper::Runtime, RuntimeHelper::PPI>::expect_best_header_hash(
						bridged_para_id,
						parachain_head_hash,
					),
				),
				(
					BridgeMessagesCall::<RuntimeHelper::Runtime, RuntimeHelper::MPI>::receive_messages_proof {
						relayer_id_at_bridged_chain,
						proof: message_proof,
						messages_count: 1,
						dispatch_weight: Weight::from_parts(1000000000, 0),
					}.into(),
					Box::new((
						helpers::VerifySubmitMessagesProofOutcome::<RuntimeHelper::Runtime, RuntimeHelper::MPI>::expect_last_delivered_nonce(
							lane_id,
							1,
						),
						helpers::VerifyRelayerRewarded::<RuntimeHelper::Runtime>::expect_relayer_reward(
							relayer_id_at_this_chain,
							RewardsAccountParams::new(
								lane_id,
								bridged_chain_id,
								RewardsAccountOwner::ThisChain,
							),
						),
					)),
				),
			]
		},
	);
}

/// Test-case makes sure that Runtime can dispatch XCM messages submitted by relayer,
/// with proofs (finality, para heads, message) batched together in signed extrinsic.
/// Also verifies relayer transaction signed extensions work as intended.
pub fn complex_relay_extrinsic_works<RuntimeHelper>(
	collator_session_key: CollatorSessionKeys<RuntimeHelper::Runtime>,
	runtime_para_id: u32,
	bridged_para_id: u32,
	sibling_parachain_id: u32,
	bridged_chain_id: bp_runtime::ChainId,
	local_relay_chain_id: NetworkId,
	lane_id: LaneId,
	prepare_configuration: impl Fn(),
	construct_and_apply_extrinsic: fn(
		sp_keyring::AccountKeyring,
		<RuntimeHelper::Runtime as frame_system::Config>::RuntimeCall,
	) -> sp_runtime::DispatchOutcome,
) where
	RuntimeHelper: WithRemoteParachainHelper,
	RuntimeHelper::Runtime:
		pallet_utility::Config<RuntimeCall = RuntimeCallOf<RuntimeHelper::Runtime>>,
	AccountIdOf<RuntimeHelper::Runtime>: From<AccountId32>,
	RuntimeCallOf<RuntimeHelper::Runtime>: From<BridgeGrandpaCall<RuntimeHelper::Runtime, RuntimeHelper::GPI>>
		+ From<BridgeParachainsCall<RuntimeHelper::Runtime, RuntimeHelper::PPI>>
		+ From<BridgeMessagesCall<RuntimeHelper::Runtime, RuntimeHelper::MPI>>
		+ From<pallet_utility::Call<RuntimeHelper::Runtime>>,
	UnderlyingChainOf<MessageBridgedChain<RuntimeHelper::MB>>:
		bp_runtime::Chain<Hash = ParaHash> + Parachain,
	<RuntimeHelper::Runtime as BridgeGrandpaConfig<RuntimeHelper::GPI>>::BridgedChain:
		bp_runtime::Chain<Hash = RelayBlockHash, BlockNumber = RelayBlockNumber> + ChainWithGrandpa,
	<RuntimeHelper::Runtime as BridgeMessagesConfig<RuntimeHelper::MPI>>::SourceHeaderChain:
		SourceHeaderChain<
			MessagesProof = FromBridgedChainMessagesProof<
				HashOf<MessageBridgedChain<RuntimeHelper::MB>>,
			>,
		>,
{
	helpers::relayed_incoming_message_works::<
		RuntimeHelper::Runtime,
		RuntimeHelper::AllPalletsWithoutSystem,
		RuntimeHelper::MPI,
	>(
		collator_session_key,
		runtime_para_id,
		sibling_parachain_id,
		local_relay_chain_id,
		construct_and_apply_extrinsic,
		|relayer_id_at_this_chain,
		 relayer_id_at_bridged_chain,
		 message_destination,
		 message_nonce,
		 xcm| {
			let para_header_number = 5;
			let relay_header_number = 1;

			prepare_configuration();

			// start with bridged relay chain block#0
			helpers::initialize_bridge_grandpa_pallet::<RuntimeHelper::Runtime, RuntimeHelper::GPI>(
				test_data::initialization_data::<RuntimeHelper::Runtime, RuntimeHelper::GPI>(0),
			);

			// generate bridged relay chain finality, parachain heads and message proofs,
			// to be submitted by relayer to this chain.
			let (
				relay_chain_header,
				grandpa_justification,
				parachain_head,
				parachain_heads,
				para_heads_proof,
				message_proof,
			) = test_data::from_parachain::make_complex_relayer_delivery_proofs::<
				<RuntimeHelper::Runtime as BridgeGrandpaConfig<RuntimeHelper::GPI>>::BridgedChain,
				RuntimeHelper::MB,
				(),
			>(
				lane_id,
				xcm.into(),
				message_nonce,
				message_destination,
				para_header_number,
				relay_header_number,
				bridged_para_id,
			);

			let parachain_head_hash = parachain_head.hash();
			let relay_chain_header_hash = relay_chain_header.hash();
			let relay_chain_header_number = *relay_chain_header.number();
			vec![(
				pallet_utility::Call::<RuntimeHelper::Runtime>::batch_all {
					calls: vec![
						BridgeGrandpaCall::<RuntimeHelper::Runtime, RuntimeHelper::GPI>::submit_finality_proof {
							finality_target: Box::new(relay_chain_header),
							justification: grandpa_justification,
						}.into(),
						BridgeParachainsCall::<RuntimeHelper::Runtime, RuntimeHelper::PPI>::submit_parachain_heads {
							at_relay_block: (relay_chain_header_number, relay_chain_header_hash),
							parachains: parachain_heads,
							parachain_heads_proof: para_heads_proof,
						}.into(),
						BridgeMessagesCall::<RuntimeHelper::Runtime, RuntimeHelper::MPI>::receive_messages_proof {
							relayer_id_at_bridged_chain,
							proof: message_proof,
							messages_count: 1,
							dispatch_weight: Weight::from_parts(1000000000, 0),
						}.into(),
					],
				}
				.into(),
				Box::new((
					helpers::VerifySubmitGrandpaFinalityProofOutcome::<
						RuntimeHelper::Runtime,
						RuntimeHelper::GPI,
					>::expect_best_header_hash(relay_chain_header_hash),
					helpers::VerifySubmitParachainHeaderProofOutcome::<
						RuntimeHelper::Runtime,
						RuntimeHelper::PPI,
					>::expect_best_header_hash(bridged_para_id, parachain_head_hash),
					helpers::VerifySubmitMessagesProofOutcome::<
						RuntimeHelper::Runtime,
						RuntimeHelper::MPI,
					>::expect_last_delivered_nonce(lane_id, 1),
					helpers::VerifyRelayerRewarded::<RuntimeHelper::Runtime>::expect_relayer_reward(
						relayer_id_at_this_chain,
						RewardsAccountParams::new(
							lane_id,
							bridged_chain_id,
							RewardsAccountOwner::ThisChain,
						),
					),
				)),
			)]
		},
	);
}

/// Estimates transaction fee for default message delivery transaction (batched with required
/// proofs) from bridged parachain.
pub fn can_calculate_fee_for_complex_message_delivery_transaction<RuntimeHelper>(
	collator_session_key: CollatorSessionKeys<RuntimeHelper::Runtime>,
	compute_extrinsic_fee: fn(pallet_utility::Call<RuntimeHelper::Runtime>) -> u128,
) -> u128
where
	RuntimeHelper: WithRemoteParachainHelper,
	RuntimeHelper::Runtime:
		pallet_utility::Config<RuntimeCall = RuntimeCallOf<RuntimeHelper::Runtime>>,
	RuntimeCallOf<RuntimeHelper::Runtime>: From<BridgeGrandpaCall<RuntimeHelper::Runtime, RuntimeHelper::GPI>>
		+ From<BridgeParachainsCall<RuntimeHelper::Runtime, RuntimeHelper::PPI>>
		+ From<BridgeMessagesCall<RuntimeHelper::Runtime, RuntimeHelper::MPI>>,
	UnderlyingChainOf<MessageBridgedChain<RuntimeHelper::MB>>:
		bp_runtime::Chain<Hash = ParaHash> + Parachain,
	<RuntimeHelper::Runtime as BridgeGrandpaConfig<RuntimeHelper::GPI>>::BridgedChain:
		bp_runtime::Chain<Hash = RelayBlockHash, BlockNumber = RelayBlockNumber> + ChainWithGrandpa,
	<RuntimeHelper::Runtime as BridgeMessagesConfig<RuntimeHelper::MPI>>::SourceHeaderChain:
		SourceHeaderChain<
			MessagesProof = FromBridgedChainMessagesProof<
				HashOf<MessageBridgedChain<RuntimeHelper::MB>>,
			>,
		>,
{
	run_test::<RuntimeHelper::Runtime, _>(collator_session_key, 1000, vec![], || {
		// generate bridged relay chain finality, parachain heads and message proofs,
		// to be submitted by relayer to this chain.
		//
		// we don't care about parameter values here, apart from the XCM message size. But we
		// do not need to have a large message here, because we're charging for every byte of
		// the message additionally
		let (
			relay_chain_header,
			grandpa_justification,
			_,
			parachain_heads,
			para_heads_proof,
			message_proof,
		) = test_data::from_parachain::make_complex_relayer_delivery_proofs::<
			<RuntimeHelper::Runtime as pallet_bridge_grandpa::Config<RuntimeHelper::GPI>>::BridgedChain,
			RuntimeHelper::MB,
			(),
		>(
			LaneId::default(),
			vec![Instruction::<()>::ClearOrigin; 1_024].into(),
			1,
			[GlobalConsensus(Polkadot), Parachain(1_000)].into(),
			1,
			5,
			1_000,
		);

		// generate batch call that provides finality for bridged relay and parachains + message
		// proof
		let batch = test_data::from_parachain::make_complex_relayer_delivery_batch::<
			RuntimeHelper::Runtime,
			RuntimeHelper::GPI,
			RuntimeHelper::PPI,
			RuntimeHelper::MPI,
			_,
		>(
			relay_chain_header,
			grandpa_justification,
			parachain_heads,
			para_heads_proof,
			message_proof,
			helpers::relayer_id_at_bridged_chain::<RuntimeHelper::Runtime, RuntimeHelper::MPI>(),
		);
		let estimated_fee = compute_extrinsic_fee(batch);

		log::error!(
			target: "bridges::estimate",
			"Estimate fee: {:?} for single message delivery for runtime: {:?}",
			estimated_fee,
			<RuntimeHelper::Runtime as frame_system::Config>::Version::get(),
		);

		estimated_fee
	})
}

/// Estimates transaction fee for default message confirmation transaction (batched with required
/// proofs) from bridged parachain.
pub fn can_calculate_fee_for_complex_message_confirmation_transaction<RuntimeHelper>(
	collator_session_key: CollatorSessionKeys<RuntimeHelper::Runtime>,
	compute_extrinsic_fee: fn(pallet_utility::Call<RuntimeHelper::Runtime>) -> u128,
) -> u128
where
	RuntimeHelper: WithRemoteParachainHelper,
	AccountIdOf<RuntimeHelper::Runtime>: From<AccountId32>,
	RuntimeHelper::Runtime:
		pallet_utility::Config<RuntimeCall = RuntimeCallOf<RuntimeHelper::Runtime>>,
	MessageThisChain<RuntimeHelper::MB>:
		bp_runtime::Chain<AccountId = AccountIdOf<RuntimeHelper::Runtime>>,
	RuntimeCallOf<RuntimeHelper::Runtime>: From<BridgeGrandpaCall<RuntimeHelper::Runtime, RuntimeHelper::GPI>>
		+ From<BridgeParachainsCall<RuntimeHelper::Runtime, RuntimeHelper::PPI>>
		+ From<BridgeMessagesCall<RuntimeHelper::Runtime, RuntimeHelper::MPI>>,
	UnderlyingChainOf<MessageBridgedChain<RuntimeHelper::MB>>:
		bp_runtime::Chain<Hash = ParaHash> + Parachain,
	<RuntimeHelper::Runtime as BridgeGrandpaConfig<RuntimeHelper::GPI>>::BridgedChain:
		bp_runtime::Chain<Hash = RelayBlockHash, BlockNumber = RelayBlockNumber> + ChainWithGrandpa,
	<RuntimeHelper::Runtime as BridgeMessagesConfig<RuntimeHelper::MPI>>::TargetHeaderChain:
		TargetHeaderChain<
			XcmAsPlainPayload,
			AccountIdOf<RuntimeHelper::Runtime>,
			MessagesDeliveryProof = FromBridgedChainMessagesDeliveryProof<
				HashOf<UnderlyingChainOf<MessageBridgedChain<RuntimeHelper::MB>>>,
			>,
		>,
{
	run_test::<RuntimeHelper::Runtime, _>(collator_session_key, 1000, vec![], || {
		// generate bridged relay chain finality, parachain heads and message proofs,
		// to be submitted by relayer to this chain.
		let unrewarded_relayers = UnrewardedRelayersState {
			unrewarded_relayer_entries: 1,
			total_messages: 1,
			..Default::default()
		};
		let (
			relay_chain_header,
			grandpa_justification,
			_,
			parachain_heads,
			para_heads_proof,
			message_delivery_proof,
		) = test_data::from_parachain::make_complex_relayer_confirmation_proofs::<
			<RuntimeHelper::Runtime as BridgeGrandpaConfig<RuntimeHelper::GPI>>::BridgedChain,
			RuntimeHelper::MB,
			(),
		>(
			LaneId::default(),
			1,
			5,
			1_000,
			AccountId32::from(Alice.public()).into(),
			unrewarded_relayers.clone(),
		);

		// generate batch call that provides finality for bridged relay and parachains + message
		// proof
		let batch = test_data::from_parachain::make_complex_relayer_confirmation_batch::<
			RuntimeHelper::Runtime,
			RuntimeHelper::GPI,
			RuntimeHelper::PPI,
			RuntimeHelper::MPI,
		>(
			relay_chain_header,
			grandpa_justification,
			parachain_heads,
			para_heads_proof,
			message_delivery_proof,
			unrewarded_relayers,
		);
		let estimated_fee = compute_extrinsic_fee(batch);

		log::error!(
			target: "bridges::estimate",
			"Estimate fee: {:?} for single message confirmation for runtime: {:?}",
			estimated_fee,
			<RuntimeHelper::Runtime as frame_system::Config>::Version::get(),
		);

		estimated_fee
	})
}
