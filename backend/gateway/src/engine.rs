use crate::domain::{MatchResult, OrderBook, RestingOrder};
use shared::domain::{Address, MarketId, OrderId, OrderSide};
use std::collections::HashMap;
use tokio::sync::mpsc::UnboundedSender;
use tracing::{error, info};

/// Commands sent from the async API layer to the sync matcher thread.
#[derive(Debug)]
pub enum MatchCommand {
    PlaceOrder {
        market_id: MarketId,
        position_id: alloy::primitives::U256,
        order_id: OrderId,
        maker: Address,
        price: u64,
        amount: u64,
        side: OrderSide,
        nonce: u64,
        deadline: u64,
    },
    CancelOrder {
        position_id: alloy::primitives::U256,
        order_id: OrderId,
    },
    #[allow(dead_code)]
    CancelAll {
        market_id: MarketId,
        position_id: alloy::primitives::U256,
        maker: Address,
    },
    Shutdown,
}

/// Run the matcher thread. Owns all order books. No tokio, no locks, no I/O.
pub fn run_matcher(
    rx: crossbeam::channel::Receiver<MatchCommand>,
    result_tx: UnboundedSender<MatchResult>,
    initial_books: HashMap<alloy::primitives::U256, OrderBook>,
) {
    let mut books: HashMap<alloy::primitives::U256, OrderBook> = initial_books;

    info!("matcher thread started, {} books loaded", books.len());

    while let Ok(cmd) = rx.recv() {
        match cmd {
            MatchCommand::Shutdown => break,
            MatchCommand::PlaceOrder {
                market_id,
                position_id,
                order_id,
                maker,
                price,
                amount,
                side,
                nonce,
                deadline,
            } => {
                let book = books.entry(position_id).or_default();

                let (fills, fully_filled) = match side {
                    OrderSide::Buy => book.match_buy(order_id, maker, price, amount),
                    OrderSide::Sell => book.match_sell(order_id, maker, price, amount),
                };

                let resting = if !fully_filled {
                    let mut resting_order = RestingOrder {
                        order_id,
                        maker,
                        price,
                        amount,
                        filled: amount - fills.iter().map(|f| f.amount).sum::<u64>(),
                        side,
                        seq: 0,
                        nonce,
                        deadline,
                    };
                    book.insert(&mut resting_order);
                    Some(resting_order)
                } else {
                    None
                };

                book.prune();

                let result = MatchResult {
                    market_id,
                    fills,
                    resting,
                    fully_filled,
                };

                if result_tx.send(result).is_err() {
                    error!("result channel closed, matcher exiting");
                    break;
                }
            }
            MatchCommand::CancelOrder {
                position_id,
                order_id,
            } => {
                if let Some(book) = books.get_mut(&position_id) {
                    book.cancel(order_id);
                    book.prune();
                }
            }
            MatchCommand::CancelAll {
                market_id,
                position_id,
                maker,
            } => {
                let removed = if let Some(book) = books.get_mut(&position_id) {
                    let r = book.cancel_all(maker);
                    book.prune();
                    r
                } else {
                    Vec::new()
                };
                info!(
                    "cancel_all: removed {} orders for maker {} in market {}",
                    removed.len(),
                    maker,
                    market_id
                );
            }
        }
    }

    info!("matcher thread exiting");
}
