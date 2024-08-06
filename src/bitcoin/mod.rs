//! Interface to the Bitcoin network.
//!
//! Broadcast transactions, poll for new unspent coins, gather fee estimates.

pub mod d;
pub mod poller;

use crate::{
    bitcoin::d::{BitcoindError, CachedTxGetter, LSBlockEntry},
    descriptors,
};
pub use d::{MempoolEntry, SyncProgress};

use std::{fmt, sync};

use miniscript::bitcoin::{self, address, bip32::ChildNumber};

const COINBASE_MATURITY: i32 = 100;

/// Information about a block
#[derive(Debug, Clone, Eq, PartialEq, Copy)]
pub struct Block {
    pub hash: bitcoin::BlockHash,
    pub height: i32,
    pub time: u32,
}

/// Information about the best block in the chain
#[derive(Debug, Clone, Eq, PartialEq, Copy)]
pub struct BlockChainTip {
    pub hash: bitcoin::BlockHash,
    pub height: i32,
}

impl fmt::Display for BlockChainTip {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "({},{})", self.height, self.hash)
    }
}

/// Our Bitcoin backend.
pub trait BitcoinInterface: Send {
    fn genesis_block_timestamp(&self) -> u32;

    fn genesis_block(&self) -> BlockChainTip;

    /// Get the progress of the block chain synchronization.
    /// Returns a rounded up percentage between 0 and 1. Use the `is_synced` method to be sure the
    /// backend is completely synced to the best known tip.
    fn sync_progress(&self) -> SyncProgress;

    /// Get the best block info.
    fn chain_tip(&self) -> BlockChainTip;

    /// Get the timestamp set in the best block's header.
    fn tip_time(&self) -> Option<u32>;

    /// Check whether this former tip is part of the current best chain.
    fn is_in_chain(&self, tip: &BlockChainTip) -> bool;

    /// Sync the wallet with the current best chain.
    /// `receive_index` and `change_index` are the last derivation indices
    /// that are expected to have been used by the wallet.
    /// In case there has been a reorg, returns the common ancestor between
    /// the wallet and the reorged chain.
    fn sync_wallet(
        &mut self,
        receive_index: ChildNumber,
        change_index: ChildNumber,
    ) -> Result<Option<BlockChainTip>, String>;

    /// Get coins received since the specified tip.
    fn received_coins(
        &self,
        tip: &BlockChainTip,
        descs: &[descriptors::SinglePathLianaDesc],
    ) -> Vec<UTxO>;

    /// Get all coins that were confirmed, and at what height and time. Along with "expired"
    /// unconfirmed coins (for instance whose creating transaction may have been replaced).
    fn confirmed_coins(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> (Vec<(bitcoin::OutPoint, i32, u32)>, Vec<bitcoin::OutPoint>);

    /// Get all coins that are being spent, and the spending txid.
    fn spending_coins(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> Vec<(bitcoin::OutPoint, bitcoin::Txid)>;

    /// Get all coins that are spent with the final spend tx txid and blocktime. Along with the
    /// coins for which the spending transaction "expired" (a conflicting transaction was mined and
    /// it wasn't spending this coin).
    fn spent_coins(
        &self,
        outpoints: &[(bitcoin::OutPoint, bitcoin::Txid)],
    ) -> (
        Vec<(bitcoin::OutPoint, bitcoin::Txid, Block)>,
        Vec<bitcoin::OutPoint>,
    );

    /// Get the common ancestor between the Bitcoin backend's tip and the given tip.
    fn common_ancestor(&self, tip: &BlockChainTip) -> Option<BlockChainTip>;

    /// Broadcast this transaction to the Bitcoin P2P network
    fn broadcast_tx(&self, tx: &bitcoin::Transaction) -> Result<(), String>;

    /// Trigger a rescan of the block chain for transactions related to this descriptor since
    /// the given date.
    fn start_rescan(
        &self,
        desc: &descriptors::LianaDescriptor,
        timestamp: u32,
    ) -> Result<(), String>;

    /// Rescan progress percentage. Between 0 and 1.
    fn rescan_progress(&self) -> Option<f64>;

    /// Get the last block chain tip with a timestamp below this. Timestamp must be a valid block
    /// timestamp.
    fn block_before_date(&self, timestamp: u32) -> Option<BlockChainTip>;

    /// Get a transaction related to the wallet along with potential confirmation info.
    fn wallet_transaction(
        &self,
        txid: &bitcoin::Txid,
    ) -> Option<(bitcoin::Transaction, Option<Block>)>;

    /// Get the details of unconfirmed transactions spending these outpoints, if any.
    fn mempool_spenders(&self, outpoints: &[bitcoin::OutPoint]) -> Vec<MempoolEntry>;

    /// Get mempool data for the given transaction.
    ///
    /// Returns `None` if the transaction is not in the mempool.
    fn mempool_entry(&self, txid: &bitcoin::Txid) -> Option<MempoolEntry>;
}

impl BitcoinInterface for d::BitcoinD {
    fn genesis_block_timestamp(&self) -> u32 {
        self.get_block_stats(
            self.get_block_hash(0)
                .expect("Genesis block hash must always be there"),
        )
        .expect("Genesis block must always be there")
        .time
    }

    fn genesis_block(&self) -> BlockChainTip {
        let height = 0;
        let hash = self
            .get_block_hash(height)
            .expect("Genesis block hash must always be there");
        BlockChainTip { hash, height }
    }

    fn sync_progress(&self) -> SyncProgress {
        self.sync_progress()
    }

    fn chain_tip(&self) -> BlockChainTip {
        self.chain_tip()
    }

    fn is_in_chain(&self, tip: &BlockChainTip) -> bool {
        self.get_block_hash(tip.height)
            .map(|bh| bh == tip.hash)
            .unwrap_or(false)
    }

    // The watchonly wallet handles this for us.
    fn sync_wallet(
        &mut self,
        _receive_index: ChildNumber,
        _change_index: ChildNumber,
    ) -> Result<Option<BlockChainTip>, String> {
        Ok(None)
    }

    fn received_coins(
        &self,
        tip: &BlockChainTip,
        descs: &[descriptors::SinglePathLianaDesc],
    ) -> Vec<UTxO> {
        let lsb_res = self.list_since_block(&tip.hash);

        lsb_res
            .received_coins
            .into_iter()
            .filter_map(|entry| {
                let LSBlockEntry {
                    outpoint,
                    amount,
                    block_height,
                    address,
                    parent_descs,
                    is_immature,
                } = entry;
                if parent_descs
                    .iter()
                    .any(|parent_desc| descs.iter().any(|desc| desc == parent_desc))
                {
                    Some(UTxO {
                        outpoint,
                        amount,
                        block_height,
                        address,
                        is_immature,
                    })
                } else {
                    None
                }
            })
            .collect()
    }

    fn confirmed_coins(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> (Vec<(bitcoin::OutPoint, i32, u32)>, Vec<bitcoin::OutPoint>) {
        // The confirmed and expired coins to be returned.
        let mut confirmed = Vec::with_capacity(outpoints.len());
        let mut expired = Vec::new();
        // Cached calls to `gettransaction`.
        let mut tx_getter = CachedTxGetter::new(self);

        for op in outpoints {
            let res = if let Some(res) = tx_getter.get_transaction(&op.txid) {
                res
            } else {
                log::error!("Transaction not in wallet for coin '{}'.", op);
                continue;
            };

            // If the transaction was confirmed, mark the coin as such.
            if let Some(block) = res.block {
                // Do not mark immature coinbase deposits as confirmed until they become mature.
                if res.is_coinbase && res.confirmations < COINBASE_MATURITY {
                    log::debug!("Coin at '{}' comes from an immature coinbase transaction with {} confirmations. Not marking it as confirmed for now.", op, res.confirmations);
                    continue;
                }
                confirmed.push((*op, block.height, block.time));
                continue;
            }

            // If the transaction was dropped from the mempool, discard the coin.
            if !self.is_in_mempool(&op.txid) {
                expired.push(*op);
            }
        }

        (confirmed, expired)
    }

    fn spending_coins(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> Vec<(bitcoin::OutPoint, bitcoin::Txid)> {
        let mut spent = Vec::with_capacity(outpoints.len());

        for op in outpoints {
            if self.is_spent(op) {
                let spending_txid = if let Some(txid) = self.get_spender_txid(op) {
                    txid
                } else {
                    // TODO: better handling of this edge case.
                    log::error!(
                        "Could not get spender of '{}'. Not reporting it as spending.",
                        op
                    );
                    continue;
                };

                spent.push((*op, spending_txid));
            }
        }

        spent
    }

    fn spent_coins(
        &self,
        outpoints: &[(bitcoin::OutPoint, bitcoin::Txid)],
    ) -> (
        Vec<(bitcoin::OutPoint, bitcoin::Txid, Block)>,
        Vec<bitcoin::OutPoint>,
    ) {
        // Spend coins to be returned.
        let mut spent = Vec::with_capacity(outpoints.len());
        // Coins whose spending transaction isn't in our local mempool anymore.
        let mut expired = Vec::new();
        // Cached calls to `gettransaction`.
        let mut tx_getter = CachedTxGetter::new(self);

        for (op, txid) in outpoints {
            let res = if let Some(res) = tx_getter.get_transaction(txid) {
                res
            } else {
                log::error!("Could not get tx {} spending coin {}.", txid, op);
                continue;
            };

            // If the transaction was confirmed, mark it as such.
            if let Some(block) = res.block {
                spent.push((*op, *txid, block));
                continue;
            }

            // If a conflicting transaction was confirmed instead, replace the txid of the
            // spender for this coin with it and mark it as confirmed.
            let conflict = res.conflicting_txs.iter().find_map(|txid| {
                tx_getter.get_transaction(txid).and_then(|tx| {
                    tx.block.and_then(|block| {
                        // Being part of our watchonly wallet isn't enough, as it could be a
                        // conflicting transaction which spends a different set of coins. Make sure
                        // it does actually spend this coin.
                        tx.tx.input.iter().find_map(|txin| {
                            if &txin.previous_output == op {
                                Some((*txid, block))
                            } else {
                                None
                            }
                        })
                    })
                })
            });
            if let Some((txid, block)) = conflict {
                spent.push((*op, txid, block));
                continue;
            }

            // If the transaction was not confirmed, a conflicting transaction spending this coin
            // too wasn't mined, but still isn't in our mempool anymore, mark the spend as expired.
            if !self.is_in_mempool(txid) {
                expired.push(*op);
            }
        }

        (spent, expired)
    }

    fn common_ancestor(&self, tip: &BlockChainTip) -> Option<BlockChainTip> {
        let mut stats = self.get_block_stats(tip.hash)?;
        let mut ancestor = *tip;

        while stats.confirmations == -1 {
            stats = self.get_block_stats(stats.previous_blockhash?)?;
            ancestor = BlockChainTip {
                hash: stats.blockhash,
                height: stats.height,
            };
        }

        Some(ancestor)
    }

    fn broadcast_tx(&self, tx: &bitcoin::Transaction) -> Result<(), String> {
        match self.broadcast_tx(tx) {
            Ok(()) => Ok(()),
            Err(BitcoindError::Server(e)) => Err(e.to_string()),
            // We assume the Bitcoin backend doesn't fail, so it must be a JSONRPC error.
            Err(e) => panic!(
                "Unexpected Bitcoin error when broadcast transaction: '{}'.",
                e
            ),
        }
    }

    fn start_rescan(
        &self,
        desc: &descriptors::LianaDescriptor,
        timestamp: u32,
    ) -> Result<(), String> {
        // FIXME: in theory i think this could potentially fail to actually start the rescan.
        self.start_rescan(desc, timestamp)
            .map_err(|e| e.to_string())
    }

    fn rescan_progress(&self) -> Option<f64> {
        self.rescan_progress()
    }

    fn block_before_date(&self, timestamp: u32) -> Option<BlockChainTip> {
        self.tip_before_timestamp(timestamp)
    }

    fn tip_time(&self) -> Option<u32> {
        let tip = self.chain_tip();
        Some(self.get_block_stats(tip.hash)?.time)
    }

    fn wallet_transaction(
        &self,
        txid: &bitcoin::Txid,
    ) -> Option<(bitcoin::Transaction, Option<Block>)> {
        self.get_transaction(txid).map(|res| (res.tx, res.block))
    }

    fn mempool_spenders(&self, outpoints: &[bitcoin::OutPoint]) -> Vec<MempoolEntry> {
        self.mempool_txs_spending_prevouts(outpoints)
            .into_iter()
            .filter_map(|txid| self.mempool_entry(&txid))
            .collect()
    }

    fn mempool_entry(&self, txid: &bitcoin::Txid) -> Option<MempoolEntry> {
        self.mempool_entry(txid)
    }
}

// FIXME: do we need to repeat the entire trait implemenation? Isn't there a nicer way?
impl BitcoinInterface for sync::Arc<sync::Mutex<dyn BitcoinInterface + 'static>> {
    fn genesis_block_timestamp(&self) -> u32 {
        self.lock().unwrap().genesis_block_timestamp()
    }

    fn genesis_block(&self) -> BlockChainTip {
        self.lock().unwrap().genesis_block()
    }

    fn sync_progress(&self) -> SyncProgress {
        self.lock().unwrap().sync_progress()
    }

    fn chain_tip(&self) -> BlockChainTip {
        self.lock().unwrap().chain_tip()
    }

    fn is_in_chain(&self, tip: &BlockChainTip) -> bool {
        self.lock().unwrap().is_in_chain(tip)
    }

    fn sync_wallet(
        &mut self,
        receive_index: ChildNumber,
        change_index: ChildNumber,
    ) -> Result<Option<BlockChainTip>, String> {
        self.lock()
            .unwrap()
            .sync_wallet(receive_index, change_index)
    }

    fn received_coins(
        &self,
        tip: &BlockChainTip,
        descs: &[descriptors::SinglePathLianaDesc],
    ) -> Vec<UTxO> {
        self.lock().unwrap().received_coins(tip, descs)
    }

    fn confirmed_coins(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> (Vec<(bitcoin::OutPoint, i32, u32)>, Vec<bitcoin::OutPoint>) {
        self.lock().unwrap().confirmed_coins(outpoints)
    }

    fn spending_coins(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> Vec<(bitcoin::OutPoint, bitcoin::Txid)> {
        self.lock().unwrap().spending_coins(outpoints)
    }

    fn spent_coins(
        &self,
        outpoints: &[(bitcoin::OutPoint, bitcoin::Txid)],
    ) -> (
        Vec<(bitcoin::OutPoint, bitcoin::Txid, Block)>,
        Vec<bitcoin::OutPoint>,
    ) {
        self.lock().unwrap().spent_coins(outpoints)
    }

    fn common_ancestor(&self, tip: &BlockChainTip) -> Option<BlockChainTip> {
        self.lock().unwrap().common_ancestor(tip)
    }

    fn broadcast_tx(&self, tx: &bitcoin::Transaction) -> Result<(), String> {
        self.lock().unwrap().broadcast_tx(tx)
    }

    fn start_rescan(
        &self,
        desc: &descriptors::LianaDescriptor,
        timestamp: u32,
    ) -> Result<(), String> {
        self.lock().unwrap().start_rescan(desc, timestamp)
    }

    fn rescan_progress(&self) -> Option<f64> {
        self.lock().unwrap().rescan_progress()
    }

    fn block_before_date(&self, timestamp: u32) -> Option<BlockChainTip> {
        self.lock().unwrap().block_before_date(timestamp)
    }

    fn tip_time(&self) -> Option<u32> {
        self.lock().unwrap().tip_time()
    }

    fn wallet_transaction(
        &self,
        txid: &bitcoin::Txid,
    ) -> Option<(bitcoin::Transaction, Option<Block>)> {
        self.lock().unwrap().wallet_transaction(txid)
    }

    fn mempool_spenders(&self, outpoints: &[bitcoin::OutPoint]) -> Vec<MempoolEntry> {
        self.lock().unwrap().mempool_spenders(outpoints)
    }

    fn mempool_entry(&self, txid: &bitcoin::Txid) -> Option<MempoolEntry> {
        self.lock().unwrap().mempool_entry(txid)
    }
}

// FIXME: We could avoid this type (and all the conversions entailing allocations) if bitcoind
// exposed the derivation index from the parent descriptor in the LSB result.
#[derive(Debug, Clone)]
pub struct UTxO {
    pub outpoint: bitcoin::OutPoint,
    pub amount: bitcoin::Amount,
    pub block_height: Option<i32>,
    pub address: bitcoin::Address<address::NetworkUnchecked>,
    pub is_immature: bool,
}
