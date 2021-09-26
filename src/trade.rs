use std::cmp::{self, Reverse};

use log::{info, warn};
/// Trade with ppl
use screeps::{HasCooldown, HasStore, MarketResourceType, ResourceType, Room, Structure, StructureTerminal, find, game::{self, market::OrderType}};

pub fn get_energy(room: &Room) {
    if let Some(terminal) = room
        .find(find::STRUCTURES)
        .into_iter()
        .filter_map(|s| match s {
            Structure::Terminal(t) => Some(t),
            _ => None,
        })
        .collect::<Vec<StructureTerminal>>()
        .first()
    {
        let terminal_energy = terminal.store_used_capacity(Some(ResourceType::Energy));
        let get_target_amount = |order_amount: u32| {
            cmp::min(cmp::min(order_amount, terminal_energy), 100_000)
        };
        if terminal.cooldown() == 0 && terminal_energy < 200_000
        {
            let orders = game::market::get_all_orders(Some(MarketResourceType::Resource(
                ResourceType::Energy,
            )));
            let mut good_orders: Vec<(f64, game::market::Order)> = orders
                .into_iter()
                .filter(|o| o.order_type == OrderType::Sell && o.remaining_amount >= 1000 && o.price < 0.75)
                .filter_map(|o| {
                    if let Some(order_room_name) = o.room_name {
                        let target_amount = get_target_amount(o.remaining_amount);
                        let trans_cost = game::market::calc_transaction_cost(
                            target_amount,
                            order_room_name,
                            room.name(),
                        );
                        if trans_cost / (target_amount as f64) < 0.66 {
                            Some((trans_cost / target_amount as f64, o))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                })
                .collect();
            good_orders.sort_unstable_by(|(efficiency_a, _o), (efficiency_b, _o2)| {
                efficiency_a.partial_cmp(efficiency_b).unwrap()
            });
            if let Some((_, order)) = good_orders.first() {
                match game::market::deal(&order.id, get_target_amount(order.remaining_amount), Some(room.name())) {
                    screeps::ReturnCode::Ok => {
                        info!("Done trade for room {}: {:?}", room.name(), order);
                    },
                    ret => warn!("Market trade: Unknown return code {:?}", ret),
                }
            }
        }
    }
}
