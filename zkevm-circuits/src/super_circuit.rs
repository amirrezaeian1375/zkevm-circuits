//! The Super Circuit is a circuit that contains all the circuits of the
//! zkEVM in order to achieve two things:
//! - Check the correct integration between circuits via the shared lookup
//!   tables, to verify that the table layouts match.
//! - Allow having a single circuit setup for which a proof can be generated
//!   that would be verified under a single aggregation circuit for the first
//!   milestone.
//!
//! The current implementation contains the following circuits:
//!
//! - [x] EVM Circuit
//! - [ ] State Circuit
//! - [x] Tx Circuit
//! - [x] Bytecode Circuit
//! - [x] Copy Circuit
//! - [x] Exponentiation Circuit
//! - [ ] Keccak Circuit
//! - [ ] MPT Circuit
//! - [x] PublicInputs Circuit
//!
//! And the following shared tables, with the circuits that use them:
//!
//! - [x] Copy Table
//!   - [x] Copy Circuit
//!   - [x] EVM Circuit
//! - [x] Exponentiation Table
//!   - [x] EVM Circuit
//! - [ ] Rw Table
//!   - [ ] State Circuit
//!   - [ ] EVM Circuit
//!   - [ ] Copy Circuit
//! - [x] Tx Table
//!   - [x] Tx Circuit
//!   - [x] EVM Circuit
//!   - [x] Copy Circuit
//!   - [x] PublicInputs Circuit
//! - [x] Bytecode Table
//!   - [x] Bytecode Circuit
//!   - [x] EVM Circuit
//!   - [x] Copy Circuit
//! - [ ] Block Table
//!   - [ ] EVM Circuit
//!   - [x] PublicInputs Circuit
//! - [ ] MPT Table
//!   - [ ] MPT Circuit
//!   - [ ] State Circuit
//! - [x] Keccak Table
//!   - [ ] Keccak Circuit
//!   - [ ] EVM Circuit
//!   - [x] Bytecode Circuit
//!   - [x] Tx Circuit
//!   - [ ] MPT Circuit

use crate::bytecode_circuit::bytecode_unroller::{
    BytecodeCircuit, BytecodeCircuitConfig, BytecodeCircuitConfigArgs,
};
use crate::copy_circuit::{CopyCircuit, CopyCircuitConfig, CopyCircuitConfigArgs};
use crate::evm_circuit::{EvmCircuit, EvmCircuitConfig, EvmCircuitConfigArgs};
use crate::exp_circuit::{ExpCircuit, ExpCircuitConfig};
use crate::keccak_circuit::keccak_packed_multi::{
    KeccakCircuit, KeccakCircuitConfig, KeccakCircuitConfigArgs,
};
//use crate::pi_circuit::{PiCircuit, PiCircuitConfig, PiCircuitConfigArgs};
use crate::state_circuit::{StateCircuit, StateCircuitConfig, StateCircuitConfigArgs};
use crate::table::{
    BlockTable, BytecodeTable, CopyTable, ExpTable, KeccakTable, MptTable, RlpTable, RwTable,
    TxTable,
};

use crate::util::{log2_ceil, Challenges, SubCircuit, SubCircuitConfig};
use crate::witness::{block_convert, Block, MptUpdates, SignedTransaction};
use bus_mapping::circuit_input_builder::{CircuitInputBuilder, CircuitsParams};
use bus_mapping::mock::BlockData;
use eth_types::geth_types::GethData;
use eth_types::Field;
use halo2_proofs::{
    circuit::{Layouter, SimpleFloorPlanner, Value},
    plonk::{Circuit, ConstraintSystem, Error, Expression},
};

use crate::pi_circuit::{PiCircuit, PiCircuitConfig, PiCircuitConfigArgs};
use crate::rlp_circuit::{RlpCircuit, RlpCircuitConfig};
use crate::tx_circuit::{TxCircuit, TxCircuitConfig, TxCircuitConfigArgs};
use std::array;

/// Mock randomness used for `SuperCircuit`.
pub const MOCK_RANDOMNESS: u64 = 0x10000;
// TODO: Figure out if we can remove MAX_TXS, MAX_CALLDATA and MAX_RWS from the
// struct.

/// Configuration of the Super Circuit
#[derive(Clone)]
pub struct SuperCircuitConfig<
    F: Field,
    const MAX_TXS: usize,
    const MAX_CALLDATA: usize,
    const MAX_INNER_BLOCKS: usize,
    const MAX_RWS: usize,
> {
    block_table: BlockTable,
    mpt_table: MptTable,
    rlp_table: RlpTable,
    tx_table: TxTable,
    evm_circuit: EvmCircuitConfig<F>,
    state_circuit: StateCircuitConfig<F>,
    tx_circuit: TxCircuitConfig<F>,
    bytecode_circuit: BytecodeCircuitConfig<F>,
    copy_circuit: CopyCircuitConfig<F>,
    keccak_circuit: KeccakCircuitConfig<F>,
    pi_circuit: PiCircuitConfig<F>,
    exp_circuit: ExpCircuitConfig<F>,
    rlp_circuit: RlpCircuitConfig<F>,
}

/// The Super Circuit contains all the zkEVM circuits
#[derive(Clone, Default, Debug)]
pub struct SuperCircuit<
    F: Field,
    const MAX_TXS: usize,
    const MAX_CALLDATA: usize,
    const MAX_INNER_BLOCKS: usize,
    const MAX_RWS: usize,
> {
    /// EVM Circuit
    pub evm_circuit: EvmCircuit<F>,
    /// State Circuit
    pub state_circuit: StateCircuit<F>,
    /// Transaction Circuit
    pub tx_circuit: TxCircuit<F>,
    /// Public Input Circuit
    pub pi_circuit: PiCircuit<F>,
    /// Bytecode Circuit
    pub bytecode_circuit: BytecodeCircuit<F>,
    /// Copy Circuit
    pub copy_circuit: CopyCircuit<F>,
    /// Exp Circuit
    pub exp_circuit: ExpCircuit<F>,
    /// Keccak Circuit
    pub keccak_circuit: KeccakCircuit<F>,
    /// Rlp Circuit
    pub rlp_circuit: RlpCircuit<F, SignedTransaction>,
}

impl<
        F: Field,
        const MAX_TXS: usize,
        const MAX_CALLDATA: usize,
        const MAX_INNER_BLOCKS: usize,
        const MAX_RWS: usize,
    > SuperCircuit<F, MAX_TXS, MAX_CALLDATA, MAX_INNER_BLOCKS, MAX_RWS>
{
    /// Return the number of rows required to verify a given block
    pub fn get_num_rows_required(block: &Block<F>) -> usize {
        let num_rows_evm_circuit = {
            let mut cs = ConstraintSystem::default();
            let config = Self::configure(&mut cs);
            config.evm_circuit.get_num_rows_required(block)
        };
        let num_rows_tx_circuit = TxCircuitConfig::<F>::get_num_rows_required(MAX_TXS);
        log::debug!(
            "num_rows_evm_circuit {}, num_rows_tx_circuit {}",
            num_rows_evm_circuit,
            num_rows_tx_circuit
        );
        num_rows_evm_circuit
            .max(num_rows_tx_circuit)
            .max(block.circuits_params.max_rws)
    }
}

impl<
        F: Field,
        const MAX_TXS: usize,
        const MAX_CALLDATA: usize,
        const MAX_INNER_BLOCKS: usize,
        const MAX_RWS: usize,
    > Circuit<F> for SuperCircuit<F, MAX_TXS, MAX_CALLDATA, MAX_INNER_BLOCKS, MAX_RWS>
{
    type Config = SuperCircuitConfig<F, MAX_TXS, MAX_CALLDATA, MAX_INNER_BLOCKS, MAX_RWS>;
    type FloorPlanner = SimpleFloorPlanner;

    fn without_witnesses(&self) -> Self {
        Self::default()
    }

    fn configure(meta: &mut ConstraintSystem<F>) -> Self::Config {
        let log_circuit_info = |meta: &mut ConstraintSystem<F>, tag: &'static str| {
            log::debug!("circuit info after {}: num_fixed_columns {}, num_advice_columns {}, num_instance_columns {}, num_selectors {}, num_permutation_columns {}, degree {}, num_challenges {}, max_phase {}",
            tag,
            meta.num_fixed_columns,
            meta.num_advice_columns,
            meta.num_instance_columns,
            meta.num_selectors,
            meta.permutation.columns.len(),
            meta.degree(),
            meta.num_challenges(),
            meta.max_phase()
        );
        };

        let tx_table = TxTable::construct(meta);
        log_circuit_info(meta, "tx");
        let rw_table = RwTable::construct(meta);
        log_circuit_info(meta, "rw");
        let mpt_table = MptTable::construct(meta);
        log_circuit_info(meta, "mpt");
        let bytecode_table = BytecodeTable::construct(meta);
        log_circuit_info(meta, "bytecode");
        let block_table = BlockTable::construct(meta);
        log_circuit_info(meta, "block");
        let q_copy_table = meta.fixed_column();
        log::debug!("q_copy_table {:?}", q_copy_table);
        let copy_table = CopyTable::construct(meta, q_copy_table);
        log_circuit_info(meta, "copy");
        let exp_table = ExpTable::construct(meta);
        log_circuit_info(meta, "exp");
        let rlp_table = RlpTable::construct(meta);
        log_circuit_info(meta, "rlp");
        let keccak_table = KeccakTable::construct(meta);
        log_circuit_info(meta, "keccak");

        let power_of_randomness = array::from_fn(|i| {
            Expression::Constant(F::from(MOCK_RANDOMNESS).pow(&[1 + i as u64, 0, 0, 0]))
        });

        let challenges = Challenges::mock(
            power_of_randomness[0].clone(),
            power_of_randomness[0].clone(),
        );

        let keccak_circuit = KeccakCircuitConfig::new(
            meta,
            KeccakCircuitConfigArgs {
                keccak_table: keccak_table.clone(),
                challenges: challenges.clone(),
            },
        );
        log_circuit_info(meta, "keccak");

        let rlp_circuit = RlpCircuitConfig::configure(meta, &rlp_table, &challenges);
        log_circuit_info(meta, "rlp");
        let pi_circuit = PiCircuitConfig::new(
            meta,
            PiCircuitConfigArgs {
                max_txs: MAX_TXS,
                max_calldata: MAX_CALLDATA,
                max_inner_blocks: MAX_INNER_BLOCKS,
                block_table: block_table.clone(),
                keccak_table: keccak_table.clone(),
                tx_table: tx_table.clone(),
                challenges: challenges.clone(),
            },
        );

        log_circuit_info(meta, "pi");
        let tx_circuit = TxCircuitConfig::new(
            meta,
            TxCircuitConfigArgs {
                tx_table: tx_table.clone(),
                keccak_table: keccak_table.clone(),
                rlp_table,
                challenges: challenges.clone(),
            },
        );
        log_circuit_info(meta, "tx circuit");

        let bytecode_circuit = BytecodeCircuitConfig::new(
            meta,
            BytecodeCircuitConfigArgs {
                bytecode_table: bytecode_table.clone(),
                keccak_table: keccak_table.clone(),
                challenges: challenges.clone(),
            },
        );

        log_circuit_info(meta, "bytecode");
        let copy_circuit = CopyCircuitConfig::new(
            meta,
            CopyCircuitConfigArgs {
                tx_table: tx_table.clone(),
                rw_table,
                bytecode_table: bytecode_table.clone(),
                copy_table,
                q_enable: q_copy_table,
                challenges: challenges.clone(),
            },
        );
        log_circuit_info(meta, "copy");
        let state_circuit = StateCircuitConfig::new(
            meta,
            StateCircuitConfigArgs {
                rw_table,
                mpt_table,
                challenges,
            },
        );

        log_circuit_info(meta, "state");
        let exp_circuit = ExpCircuitConfig::new(meta, exp_table);

        log_circuit_info(meta, "exp");
        let evm_circuit = EvmCircuitConfig::new(
            meta,
            EvmCircuitConfigArgs {
                power_of_randomness,
                tx_table: tx_table.clone(),
                rw_table,
                bytecode_table,
                block_table: block_table.clone(),
                copy_table,
                keccak_table,
                exp_table,
            },
        );

        log_circuit_info(meta, "evm");

        Self::Config {
            block_table,
            mpt_table,
            tx_table,
            rlp_table,
            evm_circuit,
            state_circuit,
            copy_circuit,
            bytecode_circuit,
            keccak_circuit,
            pi_circuit,
            rlp_circuit,
            tx_circuit,
            exp_circuit,
        }
    }

    fn synthesize(
        &self,
        config: Self::Config,
        mut layouter: impl Layouter<F>,
    ) -> Result<(), Error> {
        let block = self.evm_circuit.block.as_ref().unwrap();
        let challenges = Challenges::mock(
            Value::known(block.randomness),
            Value::known(block.randomness),
        );
        let rws = &self.state_circuit.rows;

        // PI circuit had the hardcoded constants for RegionIndex of block table
        // and tx table (which are 0 and 1).
        // The reason for that is the assignment of block/tx tables are done in
        // their load() functions which however do not emit the cells.
        // To set up copy constraints between pi cells and block/tx table cells,
        // we need to construct them manually.
        config.block_table.load(
            &mut layouter,
            &block.context,
            &block.txs,
            block.circuits_params.max_inner_blocks,
            &challenges,
        )?;

        config.tx_table.load(
            &mut layouter,
            &block.txs,
            block.circuits_params.max_txs,
            &challenges,
        )?;

        config.mpt_table.load(
            &mut layouter,
            &MptUpdates::mock_from(rws),
            Value::known(block.randomness),
        )?;

        self.keccak_circuit
            .synthesize_sub(&config.keccak_circuit, &challenges, &mut layouter)?;
        self.bytecode_circuit.synthesize_sub(
            &config.bytecode_circuit,
            &challenges,
            &mut layouter,
        )?;
        self.state_circuit
            .synthesize_sub(&config.state_circuit, &challenges, &mut layouter)?;
        self.copy_circuit
            .synthesize_sub(&config.copy_circuit, &challenges, &mut layouter)?;
        self.exp_circuit
            .synthesize_sub(&config.exp_circuit, &challenges, &mut layouter)?;
        self.evm_circuit
            .synthesize_sub(&config.evm_circuit, &challenges, &mut layouter)?;
        self.rlp_circuit
            .synthesize_sub(&config.rlp_circuit, &challenges, &mut layouter)?;
        self.tx_circuit
            .synthesize_sub(&config.tx_circuit, &challenges, &mut layouter)?;
        self.pi_circuit
            .synthesize_sub(&config.pi_circuit, &challenges, &mut layouter)?;
        Ok(())
    }
}

impl<
        F: Field,
        const MAX_TXS: usize,
        const MAX_CALLDATA: usize,
        const MAX_INNER_BLOCKS: usize,
        const MAX_RWS: usize,
    > SuperCircuit<F, MAX_TXS, MAX_CALLDATA, MAX_INNER_BLOCKS, MAX_RWS>
{
    /// From the witness data, generate a SuperCircuit instance with all of the
    /// sub-circuits filled with their corresponding witnesses.
    ///
    /// Also, return with it the minimum required SRS degree for the
    /// circuit and the Public Inputs needed.
    #[allow(clippy::type_complexity)]
    pub fn build(
        geth_data: GethData,
    ) -> Result<(u32, Self, Vec<Vec<F>>, CircuitInputBuilder), bus_mapping::Error> {
        let block_data = BlockData::new_from_geth_data_with_params(
            geth_data.clone(),
            CircuitsParams {
                max_txs: MAX_TXS,
                max_calldata: MAX_CALLDATA,
                max_inner_blocks: 64,
                max_rws: MAX_RWS,
                max_bytecode: 512,
                keccak_padding: None,
            },
        );
        let mut builder = block_data.new_circuit_input_builder();
        builder
            .handle_block(&geth_data.eth_block, &geth_data.geth_traces)
            .expect("could not handle block tx");

        let ret = Self::build_from_circuit_input_builder(&builder)?;
        Ok((ret.0, ret.1, ret.2, builder))
    }

    /// From CircuitInputBuilder, generate a SuperCircuit instance with all of
    /// the sub-circuits filled with their corresponding witnesses.
    ///
    /// Also, return with it the minimum required SRS degree for the circuit and
    /// the Public Inputs needed.
    pub fn build_from_circuit_input_builder(
        builder: &CircuitInputBuilder,
    ) -> Result<(u32, Self, Vec<Vec<F>>), bus_mapping::Error> {
        let mut block = block_convert(&builder.block, &builder.code_db).unwrap();
        block.randomness = F::from(MOCK_RANDOMNESS);
        Self::build_from_witness_block(block)
    }
    /// ..
    pub fn build_from_witness_block(
        block: Block<F>,
    ) -> Result<(u32, Self, Vec<Vec<F>>), bus_mapping::Error> {
        log::debug!(
            "super circuit build_from_witness_block, circuits_params {:?}",
            block.circuits_params
        );

        const NUM_BLINDING_ROWS: usize = 64;
        let (_, rows_needed) = Self::min_num_rows_block(&block);
        let k = log2_ceil(NUM_BLINDING_ROWS + rows_needed);
        log::debug!("super circuit needs k = {}", k);

        let evm_circuit = EvmCircuit::new_from_block(&block);
        let state_circuit = StateCircuit::new_from_block(&block);
        let tx_circuit = TxCircuit::new_from_block(&block);
        let pi_circuit = PiCircuit::new_from_block(&block);
        let bytecode_circuit = BytecodeCircuit::new_from_block(&block);
        let copy_circuit = CopyCircuit::new_from_block(&block);
        let exp_circuit = ExpCircuit::new_from_block(&block);
        let keccak_circuit = KeccakCircuit::new_from_block(&block);
        let rlp_circuit = RlpCircuit::new_from_block(&block);

        let circuit = SuperCircuit::<_, MAX_TXS, MAX_CALLDATA, MAX_INNER_BLOCKS, MAX_RWS> {
            evm_circuit,
            state_circuit,
            tx_circuit,
            pi_circuit,
            bytecode_circuit,
            copy_circuit,
            exp_circuit,
            keccak_circuit,
            rlp_circuit,
        };

        let instance = circuit.instance();
        Ok((k, circuit, instance))
    }

    /// Returns suitable inputs for the SuperCircuit.
    pub fn instance(&self) -> Vec<Vec<F>> {
        let mut instance = self.pi_circuit.instance();
        // SignVerifyChip -> ECDSAChip -> MainGate instance column
        // FIXME: why two columns??
        instance.push(vec![]);

        instance
    }

    /// Return the minimum number of rows required to prove the block
    pub fn min_num_rows_block(block: &Block<F>) -> (usize, usize) {
        let evm = EvmCircuit::min_num_rows_block(block);
        let state = StateCircuit::min_num_rows_block(block);
        let bytecode = BytecodeCircuit::min_num_rows_block(block);
        let copy = CopyCircuit::min_num_rows_block(block);
        let keccak = KeccakCircuit::min_num_rows_block(block);
        let tx = TxCircuit::min_num_rows_block(block);
        let rlp = RlpCircuit::min_num_rows_block(block);
        let exp = ExpCircuit::min_num_rows_block(block);
        let pi = PiCircuit::min_num_rows_block(block);

        let rows: Vec<(usize, usize)> = vec![evm, state, bytecode, copy, keccak, tx, rlp, exp, pi];
        let (rows_without_padding, rows_with_padding): (Vec<usize>, Vec<usize>) =
            rows.into_iter().unzip();
        log::debug!(
            "subcircuit rows(without padding): {:?}",
            rows_without_padding
        );
        log::debug!("subcircuit rows(with    padding): {:?}", rows_with_padding);
        (
            itertools::max(rows_without_padding).unwrap(),
            itertools::max(rows_with_padding).unwrap(),
        )
    }
}

#[cfg(test)]
mod super_circuit_tests {
    use super::*;
    use ethers_signers::{LocalWallet, Signer};
    use halo2_proofs::dev::MockProver;
    use halo2_proofs::halo2curves::bn256::Fr;
    use log::error;
    use mock::{eth, TestContext, MOCK_CHAIN_ID};
    use rand::SeedableRng;
    use rand_chacha::ChaCha20Rng;
    use std::collections::HashMap;

    use eth_types::evm_types::OpcodeId;
    use eth_types::{address, bytecode, geth_types::GethData, Bytecode, Word};

    #[test]
    fn super_circuit_degree() {
        let mut cs = ConstraintSystem::<Fr>::default();
        SuperCircuit::<_, 1, 32, 64, 256>::configure(&mut cs);
        log::info!("super circuit degree: {}", cs.degree());
        log::info!("super circuit minimum_rows: {}", cs.minimum_rows());
        assert!(cs.degree() <= 9);
    }

    fn test_super_circuit<
        const MAX_TXS: usize,
        const MAX_CALLDATA: usize,
        const MAX_INNER_BLOCKS: usize,
        const MAX_RWS: usize,
    >(
        block: GethData,
    ) {
        let (k, circuit, instance, _) =
            SuperCircuit::<Fr, MAX_TXS, MAX_CALLDATA, MAX_INNER_BLOCKS, MAX_RWS>::build(block)
                .unwrap();
        let prover = MockProver::run(k, &circuit, instance).unwrap();
        let res = prover.verify_par();
        if let Err(err) = res {
            error!("Verification failures: {:#?}", err);
            panic!("Failed verification");
        }
    }

    fn callee_bytecode(is_return: bool, offset: u64, length: u64) -> Bytecode {
        let memory_bytes = [0x60; 10];
        let memory_address = 0;
        let memory_value = Word::from_big_endian(&memory_bytes);
        let mut code = bytecode! {
            PUSH10(memory_value)
            PUSH1(memory_address)
            MSTORE
            PUSH2(length)
            PUSH2(32u64 - u64::try_from(memory_bytes.len()).unwrap() + offset)
        };
        code.write_op(if is_return {
            OpcodeId::RETURN
        } else {
            OpcodeId::REVERT
        });
        code
    }

    fn block_1tx_deploy() -> GethData {
        let mut rng = ChaCha20Rng::seed_from_u64(2);

        let chain_id = (*MOCK_CHAIN_ID).as_u64();

        let wallet_a = LocalWallet::new(&mut rng).with_chain_id(chain_id);
        let addr_a = wallet_a.address();

        let mut wallets = HashMap::new();
        wallets.insert(wallet_a.address(), wallet_a);

        let tx_input = callee_bytecode(true, 300, 20).code();
        let mut block: GethData = TestContext::<2, 1>::new(
            None,
            |accs| {
                accs[0].address(addr_a).balance(eth(10));
            },
            |mut txs, accs| {
                txs[0].from(accs[0].address).input(tx_input.into());
            },
            |block, _tx| block.number(0xcafeu64),
        )
        .unwrap()
        .into();
        block.sign(&wallets);
        block
    }

    fn block_1tx() -> GethData {
        let mut rng = ChaCha20Rng::seed_from_u64(2);

        let chain_id = (*MOCK_CHAIN_ID).as_u64();

        let bytecode = bytecode! {
            GAS
            STOP
        };

        let wallet_a = LocalWallet::new(&mut rng).with_chain_id(chain_id);

        let addr_a = wallet_a.address();
        let addr_b = address!("0x000000000000000000000000000000000000BBBB");

        let mut wallets = HashMap::new();
        wallets.insert(wallet_a.address(), wallet_a);

        let mut block: GethData = TestContext::<2, 1>::new(
            None,
            |accs| {
                accs[0]
                    .address(addr_b)
                    .balance(Word::from(1u64 << 20))
                    .code(bytecode);
                accs[1].address(addr_a).balance(Word::from(1u64 << 20));
            },
            |mut txs, accs| {
                txs[0]
                    .from(accs[1].address)
                    .to(accs[0].address)
                    .gas(Word::from(1_000_000u64));
            },
            |block, _tx| block.number(0xcafeu64),
        )
        .unwrap()
        .into();
        block.sign(&wallets);
        block
    }

    fn block_2tx() -> GethData {
        let mut rng = ChaCha20Rng::seed_from_u64(2);

        let chain_id = (*MOCK_CHAIN_ID).as_u64();

        let bytecode = bytecode! {
            GAS
            STOP
        };

        let wallet_a = LocalWallet::new(&mut rng).with_chain_id(chain_id);

        let addr_a = wallet_a.address();
        let addr_b = address!("0x000000000000000000000000000000000000BBBB");

        let mut wallets = HashMap::new();
        wallets.insert(wallet_a.address(), wallet_a);

        let mut block: GethData = TestContext::<2, 2>::new(
            None,
            |accs| {
                accs[0]
                    .address(addr_b)
                    .balance(Word::from(1u64 << 20))
                    .code(bytecode);
                accs[1].address(addr_a).balance(Word::from(1u64 << 20));
            },
            |mut txs, accs| {
                txs[0]
                    .from(accs[1].address)
                    .to(accs[0].address)
                    .gas(Word::from(1_000_000u64));
                txs[1]
                    .from(accs[1].address)
                    .to(accs[0].address)
                    .gas(Word::from(1_000_000u64));
            },
            |block, _tx| block.number(0xcafeu64),
        )
        .unwrap()
        .into();
        block.sign(&wallets);
        block
    }

    // High memory usage test.  Run in serial with:
    // `cargo test [...] serial_ -- --ignored --test-threads 1`
    #[ignore]
    #[test]
    fn serial_test_super_circuit_1tx_1max_tx() {
        let block = block_1tx();
        const MAX_TXS: usize = 1;
        const MAX_CALLDATA: usize = 32;
        const MAX_INNER_BLOCKS: usize = 1;
        const MAX_RWS: usize = 256;
        test_super_circuit::<MAX_TXS, MAX_CALLDATA, MAX_INNER_BLOCKS, MAX_RWS>(block);
    }

    #[ignore]
    #[test]
    fn serial_test_super_circuit_1tx_deploy_2max_tx() {
        let block = block_1tx_deploy();
        const MAX_TXS: usize = 2;
        const MAX_CALLDATA: usize = 32;
        const MAX_INNER_BLOCKS: usize = 1;
        const MAX_RWS: usize = 256;
        test_super_circuit::<MAX_TXS, MAX_CALLDATA, MAX_INNER_BLOCKS, MAX_RWS>(block);
    }

    #[ignore]
    #[test]
    fn serial_test_super_circuit_1tx_2max_tx() {
        let block = block_1tx();
        const MAX_TXS: usize = 2;
        const MAX_CALLDATA: usize = 32;
        const MAX_INNER_BLOCKS: usize = 1;
        const MAX_RWS: usize = 256;
        test_super_circuit::<MAX_TXS, MAX_CALLDATA, MAX_INNER_BLOCKS, MAX_RWS>(block);
    }
    #[ignore]
    #[test]
    fn serial_test_super_circuit_2tx_4max_tx() {
        let block = block_2tx();
        const MAX_TXS: usize = 4;
        const MAX_CALLDATA: usize = 320;
        const MAX_INNER_BLOCKS: usize = 1;
        const MAX_RWS: usize = 256;
        test_super_circuit::<MAX_TXS, MAX_CALLDATA, MAX_INNER_BLOCKS, MAX_RWS>(block);
    }
    #[ignore]
    #[test]
    fn serial_test_super_circuit_2tx_2max_tx() {
        let block = block_2tx();
        const MAX_TXS: usize = 2;
        const MAX_CALLDATA: usize = 32;
        const MAX_INNER_BLOCKS: usize = 1;
        const MAX_RWS: usize = 256;
        test_super_circuit::<MAX_TXS, MAX_CALLDATA, MAX_INNER_BLOCKS, MAX_RWS>(block);
    }
}
