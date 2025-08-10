//! Provides an easy-to-use API for producing UTXO redemption claims.
//!
//! A UTXO redemption claim is a special transaction satisfying the following
//! constraints:
//!  1. It is backed by a `ProofCollection` only -- no `SingleProof`s.
//!  2. It contains only one or two outputs, the first one without time-lock and
//!     if two the second is time-locked to the earliest release date of all
//!     inputs.
//!  3. The `public_announcements` field contains the plaintext UTXO preimage to
//!     this output along with the [`GenerationReceivingAddress`] that it is
//!     linked to.
//!  4. All spendable inputs are spent. There is no change.
//!  5. All time-locked inputs are spent into the time-locked output.
//!  6. There is no UTXO notification.
//!
//! This API is analogous to [`tx_initiation/send.rs`](super::send).
//!

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::SystemTime;

use itertools::Itertools;
use rand::rng;
use rand::Rng;
use tasm_lib::prelude::Digest;
use tasm_lib::triton_vm::prelude::BFieldCodec;
use thiserror::Error;
use tokio::fs;
use tokio::fs::File;
use tokio::io;
use tokio::io::AsyncWriteExt;

use crate::api::export::ChangePolicy;
use crate::api::export::GlobalStateLock;
use crate::api::export::NativeCurrencyAmount;
use crate::api::export::Network;
use crate::api::export::RedemptionReport;
use crate::api::export::StateLock;
use crate::api::export::Timestamp;
use crate::api::export::Transaction;
use crate::api::export::TransactionDetails;
use crate::api::export::TransactionProofType;
use crate::api::redeem::redemption_report::RedemptionReportDisplayFormat;
use crate::api::tx_initiation::builder::transaction_builder::TransactionBuilder;
use crate::api::tx_initiation::builder::transaction_details_builder::TransactionDetailsBuilder;
use crate::api::tx_initiation::builder::transaction_proof_builder::TransactionProofBuilder;
use crate::api::tx_initiation::builder::tx_input_list_builder::TxInputListBuilder;
use crate::api::tx_initiation::error::CreateTxError;
use crate::models::blockchain::transaction::public_announcement::PublicAnnouncement;
use crate::models::blockchain::transaction::utxo::Coin;
use crate::models::blockchain::transaction::utxo::Utxo;
use crate::models::blockchain::transaction::utxo_triple::UtxoTriple;
use crate::models::proof_abstractions::mast_hash::MastHash;
use crate::models::proof_abstractions::tasm::program::TritonVmProofJobOptions;
use crate::models::state::wallet::address::generation_address::GenerationReceivingAddress;
use crate::models::state::wallet::transaction_output::TxOutput;
use crate::triton_vm_job_queue::vm_job_queue;
use crate::util_types::mutator_set::mutator_set_accumulator::MutatorSetAccumulator;
use crate::util_types::mutator_set::removal_record::AbsoluteIndexSet;
use crate::util_types::mutator_set::shared::BATCH_SIZE;
use crate::util_types::mutator_set::shared::CHUNK_SIZE;
use crate::util_types::mutator_set::shared::WINDOW_SIZE;

/// The number of UTXOs in the premine, as a const.
const NUM_UTXOS_IN_PREMINE: usize = 80;

/// The threshold for the lower bound on the AOCL index.
///
/// Every absolute index set defines an interval of AOCL indices that could have
/// generated it. When the minimum of this range is less than this value, the
/// UTXO cannot be reclaimed because it cannot reliably be distinguished from a
/// premine UTXO.
const ALLOWABLE_AOCL_INDEX_LOWER_BOUND: u64 = NUM_UTXOS_IN_PREMINE as u64;

/// Compute the lower bound on the AOCL leaf index from the absolute index set.
fn absolute_index_set_to_aocl_leaf_index_lower_bound(ais: AbsoluteIndexSet) -> u64 {
    let window_start_lower_bound = ais
        .to_array()
        .into_iter()
        .max()
        .unwrap()
        .next_multiple_of(u128::from(CHUNK_SIZE))
        .saturating_sub(u128::from(WINDOW_SIZE));
    let chunk_index_lower_bound = window_start_lower_bound / u128::from(CHUNK_SIZE);
    let aocl_leaf_index_lower_bound = chunk_index_lower_bound * u128::from(BATCH_SIZE) + 1;
    aocl_leaf_index_lower_bound
        .try_into()
        .expect("AOCL leaf indices are guaranteed to fit in u64s")
}

/// provides a redeem_utxos() method to produce the UTXO redemption transaction.
#[derive(Debug, Clone)]
pub struct Redeemer {
    global_state_lock: GlobalStateLock,
}

impl From<GlobalStateLock> for Redeemer {
    fn from(global_state_lock: GlobalStateLock) -> Self {
        Self { global_state_lock }
    }
}

#[derive(Debug, Clone, Error)]
pub enum RedeemError {
    #[error("transaction initiation is disabled")]
    TransactionInitiationDisabled,
    #[error("node's proving capability is too weak")]
    ProverCapabilityTooWeak,
    #[error("create transaction error: {0}")]
    CreateTxError(CreateTxError),
}

impl From<CreateTxError> for RedeemError {
    fn from(value: CreateTxError) -> Self {
        Self::CreateTxError(value)
    }
}

impl Redeemer {
    /// Determines whether we can proceed to produce the UTXO redemption claim.
    fn can_proceed(&self) -> Result<(), RedeemError> {
        if self.global_state_lock.cli().no_transaction_initiation {
            tracing::warn!(
                "Cannot produce UTXO redemption claim because `--no-transaction-initiation` flag is set."
            );
            return Err(RedeemError::TransactionInitiationDisabled);
        }

        let capability = self.global_state_lock.cli().proving_capability();
        let proof_type = TransactionProofType::ProofCollection;
        let network = self.global_state_lock.cli().network;
        if !network.use_mock_proof() && !capability.can_prove(proof_type) {
            tracing::warn!(
                "Cannot produce UTXO redemption claim because transaction proving capability is too weak."
            );
            return Err(RedeemError::ProverCapabilityTooWeak);
        }
        Ok(())
    }

    /// Produce a UTXO redemption claim.
    ///
    /// Producing a UTXO redemption claim is an expensive task as it involves
    /// producing many proofs. It is therefore advisable to spawn the task in
    /// the background and return to the user interface server (RPC server) as
    /// quickly as possible and let the task run on its own schedule.
    //
    // This function is a spawn wrapper around [`Self::assemble_data`].
    pub fn start_redeeming_utxos(
        self,
        directory: PathBuf,
        address: Option<GenerationReceivingAddress>,
        timestamp: Timestamp,
        chunk_size: Option<usize>,
    ) -> Result<(), RedeemError> {
        self.can_proceed()?;

        let _handle = tokio::task::spawn(async move {
            self.redeem_utxos(directory, address, timestamp, chunk_size)
                .await;
        });

        Ok(())
    }

    /// Produce a UTXO redemption claim.
    async fn redeem_utxos(
        self,
        directory: PathBuf,
        address: Option<GenerationReceivingAddress>,
        timestamp: Timestamp,
        chunk_size: Option<usize>,
    ) {
        tracing::info!("Started producing UTXO redemption claim ...");
        let tx_details_list = match self.assemble_data(address, timestamp, chunk_size).await {
            Ok(txd) => txd,
            Err(e) => {
                tracing::error!("Could not assemble data for UTXO redemption claim: {}", e);
                return;
            }
        };

        tracing::info!("Obtained TX details for UTXO redemption claim; proceeding to assembling proof collections ...");
        let job_options = self.global_state_lock.cli().as_proof_job_options();
        let num_redemption_claims = tx_details_list.len();
        for (i, tx_details) in tx_details_list.into_iter().enumerate() {
            tracing::info!("starting redemption claim {i}/{num_redemption_claims}");
            Self::worker(directory.clone(), tx_details, job_options.clone()).await;
        }
        tracing::info!(
            "finished producing {num_redemption_claims}; check {}",
            directory.to_string_lossy()
        );
    }

    async fn assemble_data(
        &self,
        destination_address: Option<GenerationReceivingAddress>,
        timestamp: Timestamp,
        chunk_size: Option<usize>,
    ) -> Result<Vec<TransactionDetails>, RedeemError> {
        let gsl = &self.global_state_lock;

        tracing::info!("Assembling data for UTXO redemption claim(s) ...");

        // Acquire lock. Write-lock is unnecessary because we do not need to
        // generate new addresses.
        let mut state_lock = StateLock::read_guard(gsl).await;

        let spendable_inputs = state_lock
            .gs()
            .wallet_spendable_inputs(timestamp)
            .await
            .into_iter()
            .collect_vec();
        tracing::info!(
            "found {} spendable inputs in wallet",
            spendable_inputs.len()
        );
        let claimable_inputs = spendable_inputs
            .into_iter()
            .filter(|tx_input| {
                absolute_index_set_to_aocl_leaf_index_lower_bound(tx_input.absolute_index_set())
                    >= ALLOWABLE_AOCL_INDEX_LOWER_BOUND
            })
            .collect_vec();
        tracing::info!("of which {} are claimable", claimable_inputs.len());
        let total_amount = claimable_inputs
            .iter()
            .map(|tx_input| tx_input.utxo.get_native_currency_amount())
            .sum::<NativeCurrencyAmount>();
        tracing::info!("total claimable amount at {}", total_amount);

        // Select inputs. Wipe them out. All of them.
        let all_tx_inputs = TxInputListBuilder::new()
            .spendable_inputs(claimable_inputs)
            .policy(crate::api::export::InputSelectionPolicy::ByProvidedOrder)
            .spend_amount(total_amount)
            .build()
            .into_iter()
            .collect_vec();

        let mut transaction_details_list = vec![];
        let minimum_chunk_size = 1;
        let default_chunk_size = 10;
        let chunk_size = chunk_size
            .map(|cs| usize::max(cs, minimum_chunk_size))
            .unwrap_or(default_chunk_size);
        for tx_inputs in all_tx_inputs.chunks(chunk_size) {
            // Determine time-locked amount.
            let timelocked_amount = tx_inputs
                .iter()
                .filter(|txinput| txinput.utxo.release_date().is_some())
                .map(|txinput| txinput.utxo.get_native_currency_amount())
                .sum::<NativeCurrencyAmount>();

            // Determine liquid amount.
            let liquid_amount = tx_inputs
                .iter()
                .filter(|txinput| txinput.utxo.release_date().is_none())
                .map(|txinput| txinput.utxo.get_native_currency_amount())
                .sum::<NativeCurrencyAmount>();

            // Determine release date: earliest of all time-locks (if any).
            let earliest_release_date = tx_inputs
                .iter()
                .filter_map(|tx_input| tx_input.utxo.release_date())
                .min();

            // Determine recipient address.
            let recipient = destination_address.unwrap_or_else(|| {
                state_lock
                    .gs()
                    .wallet_state
                    .wallet_entropy
                    .nth_generation_spending_key(0)
                    .to_address()
            });

            // Generate outputs. No notifications.
            let liquid_utxo = Utxo::new(
                recipient.lock_script(),
                vec![Coin::new_native_currency(liquid_amount)],
            );
            let [liquid_sender_randomness, liquid_privacy_digest, timelocked_sender_randomness, timelocked_privacy_digest] = {
                // Keep thread-unsafe RNG inside of a sync scope to avoid async
                // issues.
                let mut rng = rng();
                [
                    rng.random::<Digest>(),
                    rng.random::<Digest>(),
                    rng.random::<Digest>(),
                    rng.random::<Digest>(),
                ]
            };
            let liquid_output = TxOutput::no_notification_as_change(
                liquid_utxo,
                liquid_sender_randomness,
                liquid_privacy_digest,
            );
            let mut tx_outputs = vec![liquid_output];
            if let Some(release_date) = earliest_release_date {
                let timelocked_utxo = Utxo::new(
                    recipient.lock_script(),
                    vec![Coin::new_native_currency(timelocked_amount)],
                )
                .with_time_lock(release_date);
                let timelocked_output = TxOutput::no_notification_as_change(
                    timelocked_utxo,
                    timelocked_sender_randomness,
                    timelocked_privacy_digest,
                );
                tx_outputs.push(timelocked_output);
            }

            // Add plaintext output UTXOs as public announcements.
            let mut public_announcements = vec![];
            for tx_output in &tx_outputs {
                let utxo_triple = UtxoTriple::from(tx_output.clone());
                let public_announcement = PublicAnnouncement {
                    message: utxo_triple.encode(),
                };
                public_announcements.push(public_announcement);
            }

            // Add the receiving address as public announcement.
            public_announcements.push(PublicAnnouncement {
                message: recipient.encode(),
            });

            // Generate transaction details. No change, so no risk of changing
            // outputs.
            let transaction_details = TransactionDetailsBuilder::new()
                .inputs(tx_inputs.to_vec().into())
                .outputs(tx_outputs.into())
                .fee(NativeCurrencyAmount::coins(0))
                .change_policy(ChangePolicy::ExactChange)
                .public_announcements(public_announcements)
                .timestamp(timestamp)
                .build(&mut state_lock)
                .await
                .map_err(RedeemError::from)?;

            transaction_details_list.push(transaction_details);
        }

        tracing::info!(
            "Done assembling data for {} UTXO redemption claim(s).",
            transaction_details_list.len()
        );

        Ok(transaction_details_list)
    }

    async fn worker(
        directory: PathBuf,
        tx_details: TransactionDetails,
        mut job_options: TritonVmProofJobOptions,
    ) {
        // Contrary to `TransactionSender::send`, we choose `ProofCollection`
        // here because
        //  a) we are running in a separate task already and therefore can
        //     afford to do expensive proving; and
        //  b) we need a `ProofCollection` (and not a `PrimitiveWitness` or a
        //     `SingleProof`) for the resulting transaction.
        let target_proof_type = TransactionProofType::ProofCollection;
        job_options.job_settings.proof_type = target_proof_type;

        let witness = tx_details.primitive_witness();
        let kernel = witness.kernel.clone();

        // Generate proofs. (ProofCollection only.)
        tracing::info!("Generating proofs ...");
        let proof = match TransactionProofBuilder::new()
            .transaction_details(&tx_details)
            .primitive_witness(witness)
            .job_queue(vm_job_queue())
            .proof_job_options(job_options)
            .build()
            .await
        {
            Ok(p) => p,
            Err(e) => {
                tracing::error!("Could not generate proofs: {e}");
                return;
            }
        };
        tracing::info!("Done generating proofs.");

        // Create transaction.
        let transaction = match TransactionBuilder::new()
            .transaction_kernel(kernel)
            .transaction_proof(proof)
            .build()
        {
            Ok(t) => t,
            Err(e) => {
                tracing::error!("Could not build transaction: {e}");
                return;
            }
        };

        // Store transaction.
        let filename = transaction.kernel.mast_hash().to_hex();
        let suffix = "redeem".to_string();
        let mut path = directory;
        path.push(filename);
        path.set_extension(suffix);
        let mut file = match File::create(path.clone()).await {
            Ok(f) => f,
            Err(e) => {
                tracing::error!(
                    "Could not open or create file `{}` for writing: {e}",
                    path.to_string_lossy()
                );
                return;
            }
        };
        let serialized_transaction = match bincode::serialize(&transaction) {
            Ok(stx) => stx,
            Err(e) => {
                tracing::error!("Could not serialize transaction: {e}");
                return;
            }
        };
        match file.write_all(&serialized_transaction).await {
            Ok(_) => (),
            Err(e) => {
                tracing::error!("Could not write to file: {e}");
            }
        };

        tracing::info!(
            "UTXO redemption claim successfully produced and written to disk at `{}`.",
            path.to_string_lossy()
        );
    }

    /// Validate many UTXO redemption claims. Verify that they are valid
    /// transactions and mutually compatible. Produce a report. Store it to
    /// disk.
    pub async fn validate_redemption_and_write_report(
        mutator_set_accumulator: MutatorSetAccumulator,
        directory: PathBuf,
        format: RedemptionReportDisplayFormat,
        compressed: bool,
        report_file_name: String,
    ) {
        let mut report = match Self::validate_redemption(mutator_set_accumulator, directory).await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("{e}");
                return;
            }
        };

        if compressed {
            tracing::info!("compressing report ...");
            report = report.compressed();
        }

        tracing::info!("writing report to disk at {report_file_name} ...");
        if let Err(e) = Self::store_report_to_disk(report, format, report_file_name).await {
            tracing::error!("{e}");
        }
    }

    async fn validate_redemption(
        mutator_set_accumulator: MutatorSetAccumulator,
        directory: PathBuf,
    ) -> Result<RedemptionReport, RedemptionValidationError> {
        // read claims from disk
        tracing::info!("reading claims from {} ...", directory.to_string_lossy());
        let redemption_claims = Self::load_redemption_claims_from_directory(directory).await?;

        // validate claims individually
        tracing::info!("checking claims for validity ...");
        let mut invalid_claims = vec![];
        let network = Network::Main;
        let num_claims = redemption_claims.len();
        for (i, (file_path, transaction)) in redemption_claims.iter().enumerate() {
            tracing::info!("{i}/{num_claims}");
            if !transaction.is_valid(network).await {
                invalid_claims.push((file_path, transaction));
            }
        }
        if !invalid_claims.is_empty() {
            let file_names_of_invalid_claims = invalid_claims
                .into_iter()
                .map(|(file_path, _tx)| file_path)
                .cloned()
                .collect_vec();
            return Err(RedemptionValidationError::InvalidClaims(
                file_names_of_invalid_claims,
            ));
        }

        // validate synchronization to current mutator set hash
        tracing::info!("checking that claims are synced to current tip ...");
        let mutator_set_hash = mutator_set_accumulator.hash();
        let mut unsynced_claims = vec![];
        for (file_path, transaction) in &redemption_claims {
            if transaction.kernel.mutator_set_hash != mutator_set_hash {
                unsynced_claims.push((file_path, transaction));
            }
        }
        if !unsynced_claims.is_empty() {
            let file_names_of_unsynced_claims = unsynced_claims
                .into_iter()
                .map(|(file_path, _tx)| file_path)
                .cloned()
                .collect_vec();
            return Err(RedemptionValidationError::UnsyncedClaims(
                file_names_of_unsynced_claims,
            ));
        }

        // validate earliest possible AOCL leaf indices
        tracing::info!("checking that claims do not pertain to premine UTXOs ...");
        let all_removal_records = redemption_claims
            .iter()
            .flat_map(|(_fp, tx)| tx.kernel.inputs.clone())
            .collect_vec();
        for removal_record in &all_removal_records {
            let earliest_possible_aocl_leaf_index =
                absolute_index_set_to_aocl_leaf_index_lower_bound(removal_record.absolute_indices);
            if earliest_possible_aocl_leaf_index < ALLOWABLE_AOCL_INDEX_LOWER_BOUND {
                return Err(RedemptionValidationError::PotentialPremineClaim(
                    earliest_possible_aocl_leaf_index,
                ));
            }
        }

        // validate no collisions
        tracing::info!("verifying no-doublespends ...");
        let mut unique_absolute_index_sets = HashSet::new();
        for removal_record in &all_removal_records {
            unique_absolute_index_sets.insert(removal_record.absolute_indices);
        }
        if unique_absolute_index_sets.len() != all_removal_records.len() {
            return Err(RedemptionValidationError::MutuallyIncompatibleClaims);
        }

        // validate removal record validity
        tracing::info!("verifying removal record validity ...");
        for removal_record in &all_removal_records {
            if !mutator_set_accumulator.can_remove(removal_record) {
                return Err(RedemptionValidationError::InvalidRemovalRecord(Box::new(
                    removal_record.absolute_indices,
                )));
            }
        }

        // parse claims; validate public announcements; produce report
        tracing::info!("validation passed; producing report ...");
        let mut report = RedemptionReport::new();
        for (file_path, transaction) in redemption_claims {
            let mut utxo_triples = vec![];
            let mut destination = None;
            for (j, public_announcement) in
                transaction.kernel.public_announcements.iter().enumerate()
            {
                let address_parse_result =
                    GenerationReceivingAddress::decode(&public_announcement.message);
                let utxo_parse_result = UtxoTriple::decode(&public_announcement.message);

                if address_parse_result.is_err() && utxo_parse_result.is_err() {
                    return Err(RedemptionValidationError::UnparsablePublicAnnouncement(
                        file_path, j,
                    ));
                }

                if let Ok(utxo_triple) = utxo_parse_result {
                    if !transaction
                        .kernel
                        .outputs
                        .contains(&utxo_triple.addition_record())
                    {
                        return Err(RedemptionValidationError::UtxoNotOutput(file_path, j));
                    }
                    utxo_triples.push(*utxo_triple);
                }

                if let Ok(address) = address_parse_result {
                    destination = Some(*address);
                }
            }

            let Some(destination) = destination else {
                return Err(RedemptionValidationError::MissingDestinationAddress);
            };

            for utxo_triple in utxo_triples {
                report.add_row(
                    utxo_triple.utxo().get_native_currency_amount(),
                    utxo_triple.utxo().release_date(),
                    destination,
                );
            }
        }

        Ok(report)
    }

    async fn store_report_to_disk(
        report: RedemptionReport,
        report_format: RedemptionReportDisplayFormat,
        report_file_name: String,
    ) -> io::Result<()> {
        let mut file_handle = tokio::fs::File::create(report_file_name).await?;
        file_handle
            .write_all(report.render(report_format).as_bytes())
            .await?;
        Ok(())
    }

    async fn load_redemption_claims_from_directory(
        dir: PathBuf,
    ) -> Result<Vec<(PathBuf, Transaction)>, RedemptionValidationError> {
        let mut transactions = Vec::new();
        let mut rd = fs::read_dir(&dir).await.map_err(|e| {
            RedemptionValidationError::ReadDir(e, dir.to_string_lossy().to_string())
        })?;
        while let Some(entry) = rd
            .next_entry()
            .await
            .map_err(|e| RedemptionValidationError::ReadDir(e, dir.to_string_lossy().to_string()))?
        {
            let file_path = entry.path();
            // skip directories
            let metadata = fs::metadata(&file_path)
                .await
                .map_err(|e| RedemptionValidationError::FileRead(file_path.clone(), e))?;
            if !metadata.is_file() {
                continue;
            }

            let bytes = fs::read(&file_path)
                .await
                .map_err(|e| RedemptionValidationError::FileRead(file_path.clone(), e))?;

            let transaction = bincode::deserialize::<Transaction>(&bytes)
                .map_err(|e| RedemptionValidationError::Deserialize(file_path.clone(), e))?;

            let modification_date = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);

            transactions.push((file_path, transaction, modification_date));
        }

        transactions.sort_by_key(|(_path, _tx, time)| *time);
        let transactions = transactions
            .into_iter()
            .map(|(path, tx, _time)| (path, tx))
            .collect_vec();

        Ok(transactions)
    }
}

#[derive(Debug, Error)]
pub enum RedemptionValidationError {
    #[error("error reading directory '{1}': {0}")]
    ReadDir(std::io::Error, String),
    #[error("error reading file '{0}': {1}")]
    FileRead(PathBuf, #[source] std::io::Error),
    #[error("failed to deserialize file `{0}`: {1}")]
    Deserialize(PathBuf, #[source] Box<bincode::ErrorKind>),
    #[error("chain state is invalid: could not produce mutator set accumulator after")]
    InvalidChainState,
    #[error("invalid claims:\n{0:#?}")]
    InvalidClaims(Vec<PathBuf>),
    #[error("unsynced claims:\n{0:#?}")]
    UnsyncedClaims(Vec<PathBuf>),
    #[error("one or more claims are mutually incompatible")]
    MutuallyIncompatibleClaims,
    #[error("invalid removal record")]
    InvalidRemovalRecord(Box<AbsoluteIndexSet>),
    #[error("potential premine claim: lower bound on AOCL leaf index is {0} < num premine UTXOs")]
    PotentialPremineClaim(u64),
    #[error("public announcement {1} of {0} cannot be parsed as UTXO or address")]
    UnparsablePublicAnnouncement(PathBuf, usize),
    #[error("parsed UTXO of public announcement {1} of {0} is not present in transaction outputs")]
    UtxoNotOutput(PathBuf, usize),
    #[error("destination address is missing")]
    MissingDestinationAddress,
    #[error("failed to write report to disk: {0}")]
    FileWrite(io::Error),
}

#[cfg(test)]
mod tests {
    use std::io::ErrorKind;

    use crate::api::redeem::redemption_report::RedemptionReportDisplayFormat;
    use crate::config_models::cli_args;
    use crate::models::blockchain::block::{Block, MINING_REWARD_TIME_LOCK_PERIOD};
    use crate::models::state::tests::helper;
    use crate::models::state::wallet::wallet_entropy::WalletEntropy;
    use crate::tests::shared::blocks::{invalid_block_with_transaction, make_mock_block};
    use crate::tests::shared::globalstate::mock_genesis_global_state;
    use crate::tests::shared_tokio_runtime;
    use crate::util_types::mutator_set::get_swbf_indices;
    use macro_rules_attr::apply;
    use num_traits::CheckedSub;
    use rand::rngs::StdRng;
    use rand::SeedableRng;
    use tracing_test::traced_test;

    use super::*;

    #[traced_test]
    #[apply(shared_tokio_runtime)]
    async fn redeem_utxos_happy_path() {
        let network = Network::Main;
        let mut rng = StdRng::seed_from_u64(435745678874);
        let mut alice = mock_genesis_global_state(
            2,
            WalletEntropy::new_pseudorandom(rng.random()),
            cli_args::Args::default_with_network(network),
        )
        .await;
        let mut alice_gsl = alice.lock_guard_mut().await;
        let alice_key = alice_gsl
            .wallet_state
            .wallet_entropy
            .nth_generation_spending_key(0);
        let bob_secret = WalletEntropy::new_random();
        let bob_key = bob_secret.nth_generation_spending_key(0);
        let genesis_block = alice_gsl.chain.archival_state().get_tip().await;
        let mut current_block = genesis_block.clone();

        // stuff happens
        let num_blocks = 15;
        let a_blocks =
            helper::make_one_branch(network, &current_block, num_blocks, &bob_key, rng.random())
                .await;
        for block in &a_blocks {
            alice_gsl.set_new_tip(block.clone()).await.unwrap();
        }
        current_block = a_blocks.last().unwrap().clone();
        let timestamp_0 = current_block.header().timestamp;

        // alice mines 3 blocks
        for _ in 0..3 {
            let (block, alice_composer_expected_utxos) =
                make_mock_block(network, &current_block, None, alice_key, rng.random()).await;
            alice_gsl
                .wallet_state
                .add_expected_utxos(alice_composer_expected_utxos)
                .await;
            alice_gsl.set_new_tip(block.clone()).await.unwrap();
            current_block = block;
        }
        const LIQUID_BLOCK_SUBSIDY: NativeCurrencyAmount = NativeCurrencyAmount::coins(64);
        let wallet_status_1 = alice_gsl.get_wallet_status_for_tip().await;
        let timestamp_1 = current_block.header().timestamp;
        assert_eq!(
            alice_gsl
                .wallet_state
                .confirmed_available_balance(&wallet_status_1, timestamp_1),
            LIQUID_BLOCK_SUBSIDY * 3
        );

        // continue for a while
        let b_blocks =
            helper::make_one_branch(network, &current_block, num_blocks, &bob_key, rng.random())
                .await;
        for block in &b_blocks {
            alice_gsl.set_new_tip(block.clone()).await.unwrap();
        }
        current_block = b_blocks.last().unwrap().clone();

        // initiate transaction
        let rando = GenerationReceivingAddress::derive_from_seed(rng.random());
        drop(alice_gsl); // drop lock to free up api
        let tx_a = helper::send_coins(&mut alice, NativeCurrencyAmount::coins(10), rando).await;
        alice_gsl = alice.lock_guard_mut().await; //re-acquire lock
        let block_after_a = invalid_block_with_transaction(&current_block, tx_a);
        current_block = block_after_a;
        alice_gsl.set_new_tip(current_block.clone()).await.unwrap();

        let wallet_status_3 = alice_gsl.get_wallet_status_for_tip().await;
        let timestamp_3 = current_block.header().timestamp;
        let wallet_balance_3 = alice_gsl
            .wallet_state
            .confirmed_available_balance(&wallet_status_3, timestamp_3);
        assert_eq!(
            wallet_balance_3,
            (LIQUID_BLOCK_SUBSIDY * 3)
                .checked_sub(&NativeCurrencyAmount::coins(10))
                .unwrap(),
            "wallet balance: {wallet_balance_3}"
        );

        // continue for another while
        let c_blocks =
            helper::make_one_branch(network, &current_block, num_blocks, &bob_key, rng.random())
                .await;
        for block in &c_blocks {
            alice_gsl.set_new_tip(block.clone()).await.unwrap();
        }
        current_block = c_blocks.last().unwrap().to_owned();

        // prepare temp directory
        println!("\n\n");
        let directory = "utxo-redemption-claims-directory-temp".to_string();
        match fs::read_dir(&directory).await {
            Err(e) if e.kind() == ErrorKind::NotFound => {
                // Directory does not exist — all good
                tokio::fs::create_dir_all(directory.clone())
                    .await
                    .unwrap_or_else(|err| {
                        panic!("could not create temp direcrory for tests: {err}")
                    });
            }
            Ok(mut entries) => {
                // Directory exists and is not empty
                assert!(
                    entries.next_entry().await.unwrap().is_none(),
                    "Directory {:?} exists and is not empty!\n\
                    Panicking out of an abundance of caution, to avoid overwriting data.\n\
                    Consider deleting or renaming the directory.",
                    directory
                );
            }
            Err(e) => {
                // Some other error occurred
                panic!("Error checking directory: {:?}", e);
            }
        }

        // produce redemption claim
        let address = GenerationReceivingAddress::derive_from_seed(rng.random());
        let timestamp = current_block.header().timestamp + Timestamp::years(4);
        drop(alice_gsl); // free up api
        alice
            .api()
            .redeemer()
            .redeem_utxos(directory.clone().into(), Some(address), timestamp, Some(2))
            .await;

        // verify redemption
        let mutator_set_accumulator = alice
            .lock_guard()
            .await
            .chain
            .light_state()
            .mutator_set_accumulator_after()
            .unwrap();
        let produced_report =
            Redeemer::validate_redemption(mutator_set_accumulator, directory.clone().into())
                .await
                .unwrap()
                .compressed();

        // match the obtained report against what we expect
        let mut expected_report = RedemptionReport::new();
        expected_report.add_row(wallet_balance_3, None, address);
        expected_report.add_row(
            LIQUID_BLOCK_SUBSIDY * 3,
            // offset by 30 minutes because that's how the composer UTXOs are made
            Some(
                timestamp_0
                    + network.target_block_interval()
                    + MINING_REWARD_TIME_LOCK_PERIOD
                    + Timestamp::minutes(30),
            ),
            address,
        );

        assert_eq!(
            expected_report,
            produced_report,
            "Expected:\n\n{}\n\nProduced:\n\n{}",
            expected_report.render(RedemptionReportDisplayFormat::Readable),
            produced_report.render(RedemptionReportDisplayFormat::Readable)
        );

        // clean up the temp directory
        tokio::fs::remove_dir_all(directory)
            .await
            .unwrap_or_else(|e| panic!("could not delete temp direcrory for tests: {e}"));

        // show report in case the user wants to see
        println!(
            "{}",
            produced_report.render(RedemptionReportDisplayFormat::Readable)
        );
    }

    #[test]
    fn num_utxos_in_premine_agrees_with_const() {
        assert_eq!(
            NUM_UTXOS_IN_PREMINE,
            Block::premine_utxos(Network::Main).len()
        );
    }

    #[ignore = "informative statistics"]
    #[test]
    fn estimate_probability_that_early_honest_utxos_are_unclaimable() {
        let mut rng = rng();
        let first_honest_aocl_leaf_index = NUM_UTXOS_IN_PREMINE as u64;
        for i in 0..60 {
            let true_aocl_leaf_index = first_honest_aocl_leaf_index + i * u64::from(BATCH_SIZE);
            let mut successes = 0;
            let num_trials = 10000;
            for _ in 0..num_trials {
                let absolute_index_set = AbsoluteIndexSet::new(&get_swbf_indices(
                    rng.random(),
                    rng.random(),
                    rng.random(),
                    true_aocl_leaf_index,
                ));
                let aocl_leaf_index_lower_bound =
                    absolute_index_set_to_aocl_leaf_index_lower_bound(absolute_index_set);
                if aocl_leaf_index_lower_bound < ALLOWABLE_AOCL_INDEX_LOWER_BOUND {
                    successes += 1;
                }
            }

            println!(
                "UTXO {}-{}: {}",
                true_aocl_leaf_index.next_multiple_of(u64::from(BATCH_SIZE)) + 1
                    - u64::from(BATCH_SIZE),
                true_aocl_leaf_index.next_multiple_of(u64::from(BATCH_SIZE)),
                f64::from(successes) / f64::from(num_trials)
            );
        }
    }
}
