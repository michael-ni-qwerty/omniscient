use shared::domain::{Address, MarketId, OrderId, OrderSide};
use std::collections::{BTreeMap, VecDeque};

/// An order resting in the book.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct RestingOrder {
    pub order_id: OrderId,
    pub maker: Address,
    pub price: u64,
    pub amount: u64,
    pub filled: u64,
    pub side: OrderSide,
    pub seq: u64,
    pub nonce: u64,
    pub deadline: u64,
}

impl RestingOrder {
    pub fn remaining(&self) -> u64 {
        self.amount - self.filled
    }
}

/// A single fill produced by matching.
#[derive(Debug, Clone)]
pub struct Fill {
    pub maker_order_id: OrderId,
    pub taker_order_id: OrderId,
    pub maker: Address,
    pub taker: Address,
    pub price: u64,
    pub amount: u64,
    pub taker_side: OrderSide,
}

/// Result of processing an incoming order.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct MatchResult {
    pub market_id: MarketId,
    pub fills: Vec<Fill>,
    pub resting: Option<RestingOrder>,
    pub fully_filled: bool,
}

/// Per-position order book. Buy side: highest price first. Sell side: lowest price first.
#[derive(Debug, Default)]
pub struct OrderBook {
    buys: BTreeMap<u64, VecDeque<RestingOrder>>,
    sells: BTreeMap<u64, VecDeque<RestingOrder>>,
    next_seq: u64,
}

impl OrderBook {
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a resting order into the book.
    pub fn insert(&mut self, order: &mut RestingOrder) {
        order.seq = self.next_seq;
        self.next_seq += 1;
        match order.side {
            OrderSide::Buy => self
                .buys
                .entry(order.price)
                .or_default()
                .push_back(order.clone()),
            OrderSide::Sell => self
                .sells
                .entry(order.price)
                .or_default()
                .push_back(order.clone()),
        }
    }

    /// Match an incoming buy order against the sell side.
    /// Returns fills and whether the order was fully filled.
    pub fn match_buy(
        &mut self,
        taker_order_id: OrderId,
        taker: Address,
        price: u64,
        amount: u64,
    ) -> (Vec<Fill>, bool) {
        let mut remaining = amount;
        let mut fills = Vec::new();

        while remaining > 0 {
            let best_sell_price = match self.sells.keys().next().copied() {
                Some(p) if p <= price => p,
                _ => break,
            };

            let level = self.sells.get_mut(&best_sell_price).unwrap();
            while remaining > 0 {
                let maker = match level.front() {
                    Some(o) => o,
                    None => break,
                };
                if maker.maker == taker {
                    let _ = level.pop_front();
                    continue;
                }
                let fill_amount = remaining.min(maker.remaining());
                let maker_addr = maker.maker;
                let maker_order_id = maker.order_id;
                let fill_price = maker.price;

                let maker_order = level.front_mut().unwrap();
                maker_order.filled += fill_amount;
                if maker_order.remaining() == 0 {
                    level.pop_front();
                }

                fills.push(Fill {
                    maker_order_id,
                    taker_order_id,
                    maker: maker_addr,
                    taker,
                    price: fill_price,
                    amount: fill_amount,
                    taker_side: OrderSide::Buy,
                });
                remaining -= fill_amount;
            }
            if level.is_empty() {
                self.sells.remove(&best_sell_price);
            }
        }

        let fully_filled = remaining == 0;
        (fills, fully_filled)
    }

    /// Match an incoming sell order against the buy side.
    pub fn match_sell(
        &mut self,
        taker_order_id: OrderId,
        taker: Address,
        price: u64,
        amount: u64,
    ) -> (Vec<Fill>, bool) {
        let mut remaining = amount;
        let mut fills = Vec::new();

        while remaining > 0 {
            let best_buy_price = match self.buys.keys().next_back().copied() {
                Some(p) if p >= price => p,
                _ => break,
            };

            let level = self.buys.get_mut(&best_buy_price).unwrap();
            while remaining > 0 {
                let maker = match level.front() {
                    Some(o) => o,
                    None => break,
                };
                if maker.maker == taker {
                    let _ = level.pop_front();
                    continue;
                }
                let fill_amount = remaining.min(maker.remaining());
                let maker_addr = maker.maker;
                let maker_order_id = maker.order_id;
                let fill_price = maker.price;

                let maker_order = level.front_mut().unwrap();
                maker_order.filled += fill_amount;
                if maker_order.remaining() == 0 {
                    level.pop_front();
                }

                fills.push(Fill {
                    maker_order_id,
                    taker_order_id,
                    maker: maker_addr,
                    taker,
                    price: fill_price,
                    amount: fill_amount,
                    taker_side: OrderSide::Sell,
                });
                remaining -= fill_amount;
            }
            if level.is_empty() {
                self.buys.remove(&best_buy_price);
            }
        }

        let fully_filled = remaining == 0;
        (fills, fully_filled)
    }

    /// Cancel a resting order by order_id. Returns true if found and removed.
    pub fn cancel(&mut self, order_id: OrderId) -> bool {
        for level in self.buys.values_mut() {
            if let Some(pos) = level.iter().position(|o| o.order_id == order_id) {
                level.remove(pos);
                return true;
            }
        }
        for level in self.sells.values_mut() {
            if let Some(pos) = level.iter().position(|o| o.order_id == order_id) {
                level.remove(pos);
                return true;
            }
        }
        false
    }

    /// Cancel all resting orders for a maker. Returns the removed order IDs.
    pub fn cancel_all(&mut self, maker: Address) -> Vec<OrderId> {
        let mut removed = Vec::new();
        for level in self.buys.values_mut() {
            let mut i = 0;
            while i < level.len() {
                if level[i].maker == maker {
                    removed.push(level[i].order_id);
                    level.remove(i);
                } else {
                    i += 1;
                }
            }
        }
        for level in self.sells.values_mut() {
            let mut i = 0;
            while i < level.len() {
                if level[i].maker == maker {
                    removed.push(level[i].order_id);
                    level.remove(i);
                } else {
                    i += 1;
                }
            }
        }
        removed
    }

    /// Remove empty price levels (cleanup).
    pub fn prune(&mut self) {
        self.buys.retain(|_, v| !v.is_empty());
        self.sells.retain(|_, v| !v.is_empty());
    }

    #[allow(dead_code)]
    pub fn best_bid(&self) -> Option<u64> {
        self.buys.keys().next_back().copied()
    }

    #[allow(dead_code)]
    pub fn best_ask(&self) -> Option<u64> {
        self.sells.keys().next().copied()
    }
}

/// Compute taker fee (round up).
#[allow(dead_code)]
pub fn taker_fee(volume: u128) -> u128 {
    volume * shared::constants::TAKER_FEE_BPS as u128 / 10000
}

/// Compute maker rebate (round down).
#[allow(dead_code)]
pub fn maker_rebate(volume: u128) -> u128 {
    volume * shared::constants::MAKER_REBATE_BPS as u128 / 10000
}

/// Buy volume (round up): ceil(amount * price / SCALE).
#[allow(dead_code)]
pub fn buy_volume(amount: u64, price: u64) -> u128 {
    (amount as u128 * price as u128).div_ceil(shared::constants::PRICE_SCALE as u128)
}

/// Sell volume (round down): floor(amount * price / SCALE).
#[allow(dead_code)]
pub fn sell_volume(amount: u64, price: u64) -> u128 {
    amount as u128 * price as u128 / shared::constants::PRICE_SCALE as u128
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(n: u8) -> Address {
        Address([n; 20])
    }

    fn oid() -> OrderId {
        OrderId::new()
    }

    #[test]
    fn test_match_buy_crosses_sell() {
        let mut book = OrderBook::new();
        let mut sell = RestingOrder {
            order_id: oid(),
            maker: addr(1),
            price: 500_000,
            amount: 100,
            filled: 0,
            side: OrderSide::Sell,
            seq: 0,
            nonce: 0,
            deadline: 0,
        };
        book.insert(&mut sell);

        let (fills, fully) = book.match_buy(oid(), addr(2), 510_000, 50);
        assert_eq!(fills.len(), 1);
        assert_eq!(fills[0].amount, 50);
        assert_eq!(fills[0].price, 500_000);
        assert!(fully);
    }

    #[test]
    fn test_match_buy_no_cross() {
        let mut book = OrderBook::new();
        let mut sell = RestingOrder {
            order_id: oid(),
            maker: addr(1),
            price: 600_000,
            amount: 100,
            filled: 0,
            side: OrderSide::Sell,
            seq: 0,
            nonce: 0,
            deadline: 0,
        };
        book.insert(&mut sell);

        let (fills, fully) = book.match_buy(oid(), addr(2), 500_000, 50);
        assert!(fills.is_empty());
        assert!(!fully);
    }

    #[test]
    fn test_self_trade_prevention() {
        let mut book = OrderBook::new();
        let mut sell = RestingOrder {
            order_id: oid(),
            maker: addr(1),
            price: 500_000,
            amount: 100,
            filled: 0,
            side: OrderSide::Sell,
            seq: 0,
            nonce: 0,
            deadline: 0,
        };
        book.insert(&mut sell);

        let (fills, fully) = book.match_buy(oid(), addr(1), 510_000, 50);
        assert!(fills.is_empty());
        assert!(!fully);
    }

    #[test]
    fn test_partial_fill_rests() {
        let mut book = OrderBook::new();
        let mut sell = RestingOrder {
            order_id: oid(),
            maker: addr(1),
            price: 500_000,
            amount: 30,
            filled: 0,
            side: OrderSide::Sell,
            seq: 0,
            nonce: 0,
            deadline: 0,
        };
        book.insert(&mut sell);

        let (fills, fully) = book.match_buy(oid(), addr(2), 510_000, 50);
        assert_eq!(fills.len(), 1);
        assert_eq!(fills[0].amount, 30);
        assert!(!fully);
    }

    #[test]
    fn test_price_time_priority() {
        let mut book = OrderBook::new();
        let mut s1 = RestingOrder {
            order_id: oid(),
            maker: addr(1),
            price: 500_000,
            amount: 10,
            filled: 0,
            side: OrderSide::Sell,
            seq: 0,
            nonce: 0,
            deadline: 0,
        };
        book.insert(&mut s1);
        let mut s2 = RestingOrder {
            order_id: oid(),
            maker: addr(2),
            price: 500_000,
            amount: 10,
            filled: 0,
            side: OrderSide::Sell,
            seq: 0,
            nonce: 0,
            deadline: 0,
        };
        book.insert(&mut s2);

        let (fills, _) = book.match_buy(oid(), addr(3), 510_000, 10);
        assert_eq!(fills.len(), 1);
        assert_eq!(fills[0].maker, addr(1));
    }

    #[test]
    fn test_cancel_order() {
        let mut book = OrderBook::new();
        let order_id = oid();
        let mut order = RestingOrder {
            order_id,
            maker: addr(1),
            price: 500_000,
            amount: 100,
            filled: 0,
            side: OrderSide::Buy,
            seq: 0,
            nonce: 0,
            deadline: 0,
        };
        book.insert(&mut order);
        assert!(book.cancel(order_id));
        assert!(!book.cancel(order_id));
    }

    #[test]
    fn test_cancel_all() {
        let mut book = OrderBook::new();
        let id1 = oid();
        let id2 = oid();
        let mut o1 = RestingOrder {
            order_id: id1,
            maker: addr(1),
            price: 500_000,
            amount: 100,
            filled: 0,
            side: OrderSide::Buy,
            seq: 0,
            nonce: 0,
            deadline: 0,
        };
        book.insert(&mut o1);
        let mut o2 = RestingOrder {
            order_id: id2,
            maker: addr(1),
            price: 490_000,
            amount: 100,
            filled: 0,
            side: OrderSide::Buy,
            seq: 0,
            nonce: 0,
            deadline: 0,
        };
        book.insert(&mut o2);
        let removed = book.cancel_all(addr(1));
        assert_eq!(removed.len(), 2);
    }

    #[test]
    fn test_fee_math() {
        let vol: u128 = 1_000_000; // 1 USDC (6dp)
        let tf = taker_fee(vol);
        let mr = maker_rebate(vol);
        assert_eq!(tf, 5000); // 0.5% of 1 USDC = 0.005 USDC
        assert_eq!(mr, 1000); // 0.1% of 1 USDC = 0.001 USDC
        assert!(tf >= mr); // net fee >= 0
    }

    #[test]
    fn test_buy_volume_rounds_up() {
        // amount=3, price=333333 -> 3*333333=999999 -> ceil(999999/1e6)=1
        assert_eq!(buy_volume(3, 333_333), 1);
    }

    #[test]
    fn test_sell_volume_rounds_down() {
        // amount=3, price=333333 -> 999999 -> floor(999999/1e6)=0
        assert_eq!(sell_volume(3, 333_333), 0);
    }
}
