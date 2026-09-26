#![allow(clippy::unwrap_used)]

use maya_lanes::{Bid, Fees, Lane, Params, Tx, auction, build};

const P: Params = Params {
    capacity: 10,
    app_target: 2,
    global_target: 5,
    denom: 8,
    floor: 1,
    app_cap: 10,
};

#[test]
fn the_auction_caps_each_holder_and_the_total() {
    let lanes = auction(
        vec![
            Bid {
                holder: 1,
                slots: 10,
                price: 50,
            },
            Bid {
                holder: 2,
                slots: 2,
                price: 40,
            },
            Bid {
                holder: 3,
                slots: 5,
                price: 30,
            },
        ],
        6,
        3,
    );
    assert_eq!(
        lanes,
        vec![
            Lane {
                holder: 1,
                slots: 3,
                price: 50
            },
            Lane {
                holder: 2,
                slots: 2,
                price: 40
            },
            Lane {
                holder: 3,
                slots: 1,
                price: 30
            }
        ]
    );
    assert_eq!(
        lanes.iter().map(|l| l.slots).sum::<u64>(),
        6,
        "no one buys the whole chain"
    );
}

#[test]
fn lane_traffic_goes_first_and_unused_lane_space_is_released() {
    let fees = Fees::new(P);
    let lanes = [Lane {
        holder: 7,
        slots: 4,
        price: 1,
    }];
    let mut pool: Vec<Tx> = (0..3)
        .map(|i| Tx {
            app: 1,
            max_fee: 1,
            lane: Some(7),
            arrived: i,
        })
        .collect();
    pool.extend((0..20).map(|i| Tx {
        app: 2,
        max_fee: 100,
        lane: None,
        arrived: i,
    }));
    let block = build(&mut pool, &lanes, &fees);
    assert_eq!(block.len(), 10);
    assert_eq!(
        block.iter().filter(|t| t.lane == Some(7)).count(),
        3,
        "the lane's own traffic is in, despite paying less"
    );
    assert_eq!(
        block.iter().filter(|t| t.lane.is_none()).count(),
        7,
        "the lane's unused slot went to the general pool"
    );
}

#[test]
fn a_hot_app_raises_only_its_own_local_fee() {
    let mut fees = Fees::new(P);
    for _ in 0..20 {
        fees.update(&[(1, 5), (2, 1)].into_iter().collect());
    }
    assert!(
        fees.price(1) > 5 * fees.price(2) / 2,
        "hot app 1: {} vs quiet app 2: {}",
        fees.price(1),
        fees.price(2)
    );
    assert_eq!(fees.local[&2], 1, "the quiet app stays at the floor");
}
