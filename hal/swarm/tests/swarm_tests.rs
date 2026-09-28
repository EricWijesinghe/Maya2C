//! Robot swarms (Master Prompt 6 §8): the wire formats against their
//! reference implementations, and 100 simulated agents bidding for tasks and
//! flying committed plans whose conflicts are provable.

#![allow(clippy::unwrap_used)]

use maya_swarm::auction::{Auction, Bid, commit};
use maya_swarm::cdr::{self, Pose, PoseStamped};
use maya_swarm::mavlink::{self, Frame, GlobalPosition, Heartbeat, Message};
use maya_swarm::paths::{Plan, Waypoint, check_all, first_conflict, verify_conflict};
use serde_json::Value;

fn vectors() -> Value {
    serde_json::from_str(include_str!("fixtures/vectors.json")).unwrap()
}

fn num(v: &Value, k: &str) -> i64 {
    v["fields"][k].as_i64().unwrap()
}

#[test]
fn mavlink_matches_pymavlink_byte_for_byte() {
    for case in vectors()["mavlink"].as_array().unwrap() {
        let bytes = hex::decode(case["bytes"].as_str().unwrap()).unwrap();
        let frame = mavlink::decode(&bytes).unwrap();
        let t = |k| u8::try_from(num(case, k)).unwrap();
        let expected = match case["name"].as_str().unwrap() {
            "HEARTBEAT" => Message::Heartbeat(Heartbeat {
                kind: t("type"),
                autopilot: t("autopilot"),
                base_mode: t("base_mode"),
                custom_mode: u32::try_from(num(case, "custom_mode")).unwrap(),
                system_status: t("system_status"),
                mavlink_version: t("mavlink_version"),
            }),
            _ => Message::GlobalPosition(GlobalPosition {
                time_boot_ms: u32::try_from(num(case, "time_boot_ms")).unwrap(),
                lat: i32::try_from(num(case, "lat")).unwrap(),
                lon: i32::try_from(num(case, "lon")).unwrap(),
                alt: i32::try_from(num(case, "alt")).unwrap(),
                relative_alt: i32::try_from(num(case, "relative_alt")).unwrap(),
                vx: i16::try_from(num(case, "vx")).unwrap(),
                vy: i16::try_from(num(case, "vy")).unwrap(),
                vz: i16::try_from(num(case, "vz")).unwrap(),
                hdg: u16::try_from(num(case, "hdg")).unwrap(),
            }),
        };
        let want = Frame {
            seq: u8::try_from(case["seq"].as_u64().unwrap()).unwrap(),
            sysid: 7,
            compid: 1,
            message: expected,
        };
        assert_eq!(frame, want, "{}", case["name"]);
        assert_eq!(
            mavlink::encode(&want),
            bytes,
            "{} encodes to pymavlink's bytes",
            case["name"]
        );

        // A flipped payload bit fails the checksum; every prefix is refused.
        let mut bad = bytes.clone();
        bad[11] ^= 1;
        assert!(mavlink::decode(&bad).is_err());
        for n in 0..bytes.len() {
            assert!(mavlink::decode(&bytes[..n]).is_err());
        }
        let mut signed = bytes.clone();
        signed[2] = 1;
        assert!(
            mavlink::decode(&signed).is_err(),
            "signed frames are refused, not trusted"
        );
    }
}

fn floats(v: &Value) -> Vec<f64> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_f64().unwrap())
        .collect()
}

#[test]
fn ros2_cdr_matches_rosbags_byte_for_byte() {
    let cases = vectors()["ros2_cdr"].as_array().unwrap().clone();
    let pose_of = |c: &Value| {
        let (p, o) = (floats(&c["position"]), floats(&c["orientation"]));
        Pose {
            position: [p[0], p[1], p[2]],
            orientation: [o[0], o[1], o[2], o[3]],
        }
    };
    let pose_bytes = hex::decode(cases[0]["bytes"].as_str().unwrap()).unwrap();
    assert_eq!(cdr::decode_pose(&pose_bytes).unwrap(), pose_of(&cases[0]));
    assert_eq!(cdr::encode_pose(&pose_of(&cases[0])), pose_bytes);

    let stamped_bytes = hex::decode(cases[1]["bytes"].as_str().unwrap()).unwrap();
    let stamped = PoseStamped {
        sec: i32::try_from(cases[1]["sec"].as_i64().unwrap()).unwrap(),
        nanosec: u32::try_from(cases[1]["nanosec"].as_u64().unwrap()).unwrap(),
        frame_id: cases[1]["frame_id"].as_str().unwrap().to_owned(),
        pose: pose_of(&cases[1]),
    };
    assert_eq!(cdr::decode_pose_stamped(&stamped_bytes).unwrap(), stamped);
    assert_eq!(cdr::encode_pose_stamped(&stamped), stamped_bytes);
    for n in 0..stamped_bytes.len() {
        assert!(cdr::decode_pose_stamped(&stamped_bytes[..n]).is_err());
    }
    assert!(
        cdr::decode_pose(&[pose_bytes.clone(), vec![0]].concat()).is_err(),
        "trailing byte"
    );
}

const AGENTS: u32 = 100;
const TASKS: u32 = 60;

fn home(agent: u32) -> [i64; 3] {
    [
        i64::from(agent % 10) * 50_000,
        i64::from(agent / 10) * 50_000,
        0,
    ]
}

fn site(task: u32) -> [i64; 3] {
    // Sixty distinct sites, offset from the homes' grid.
    [
        i64::from(task % 10) * 50_000 + 25_000,
        i64::from(task / 10) * 50_000 + 25_000,
        0,
    ]
}

fn distance(a: [i64; 3], b: [i64; 3]) -> u64 {
    a.iter().zip(b).map(|(x, y)| x.abs_diff(y)).sum()
}

/// Take off to a cruise altitude of the agent's own, fly, land at the site.
fn plan(agent: u32, to: [i64; 3], cruise_mm: i64) -> Plan {
    let from = home(agent);
    let up = [from[0], from[1], cruise_mm];
    let over = [to[0], to[1], cruise_mm];
    let flight = distance(up, over); // 1 mm per ms
    Plan {
        agent,
        waypoints: vec![
            Waypoint { t_ms: 0, at: from },
            Waypoint {
                t_ms: 10_000,
                at: up,
            },
            Waypoint {
                t_ms: 10_000 + flight,
                at: over,
            },
            Waypoint {
                t_ms: 20_000 + flight,
                at: [to[0], to[1], 1_000 + i64::from(agent) * 10],
            },
        ],
    }
}

#[test]
fn a_hundred_agents_bid_fly_committed_plans_and_a_conflict_is_provable() {
    let started = std::time::Instant::now();
    // Sealed bids: every agent bids its distance to every task.
    let mut auction = Auction::new(0..TASKS);
    let mut sealed = Vec::new();
    for agent in 0..AGENTS {
        for task in 0..TASKS {
            let bid = Bid {
                agent,
                task,
                price: distance(home(agent), site(task)),
            };
            let salt = [u8::try_from(agent % 256).unwrap(); 32];
            auction.seal(agent, task, commit(&bid, &salt)).unwrap();
            sealed.push((bid, salt));
        }
    }
    auction.close();
    // One agent lies about its price at reveal time: refused.
    let (mut lie, salt) = sealed[0];
    lie.price += 1;
    assert!(auction.reveal(lie, &salt).is_err());
    for (bid, salt) in &sealed {
        auction.reveal(*bid, salt).unwrap();
    }
    let awarded = auction.award();
    assert_eq!(
        awarded.len(),
        usize::try_from(TASKS).unwrap(),
        "every task has a flyer"
    );

    // Winners commit plans at distinct cruise layers; the swarm checks clean.
    let plans: Vec<Plan> = awarded
        .iter()
        .map(|(task, (agent, _))| plan(*agent, site(*task), 20_000 + i64::from(*agent) * 3_000))
        .collect();
    for p in &plans {
        p.validate().unwrap();
    }
    let conflicts = check_all(&plans);
    assert!(conflicts.is_empty(), "{conflicts:?}");
    println!(
        "swarm (simulated): {AGENTS} agents, {} tasks awarded, {} plans checked pairwise ({} pairs) with no conflict, {:?}",
        awarded.len(),
        plans.len(),
        plans.len() * (plans.len() - 1) / 2,
        started.elapsed()
    );

    // Two agents on the same layer crossing paths: found, and provable on
    // chain from the two commitments.
    let a = plan(1, home(8), 50_000);
    let b = plan(8, home(1), 50_000);
    let t = first_conflict(&a, &b).unwrap();
    assert!(verify_conflict(&a.commitment(), &a, &b.commitment(), &b, t));
    let mut altered = b.clone();
    altered.waypoints[2].at[2] += 10_000;
    assert!(
        !verify_conflict(&a.commitment(), &a, &b.commitment(), &altered, t),
        "a plan other than the committed one proves nothing"
    );
}
