# Prior art: Swarm coordination (many robots or drones agreeing on tasks without a central controller)

**Search status:** searched 2026-11-10 (web search; sources below). The ROS 2 and MAVLink specifications were not fetched this session; their use is cited from the repo.

Feature: CATALOG §I row `swarm-coordination`, from `hal/swarm`.

## Who else does something similar

| Project or work | What it does | Source |
|---|---|---|
| Strobel, Castello Ferrer, Dorigo (Frontiers in Robotics and AI, 2020) | Ethereum smart contracts as "meta-controllers" for robot swarms; consensus protocols vs Byzantine robots and Sybil attacks in the ARGoS simulator; open-source ARGoS-Ethereum bridge | [Frontiers](https://www.frontiersin.org/articles/10.3389/frobt.2020.00054/text), [PMC](https://pmc.ncbi.nlm.nih.gov/articles/PMC7806104), [IRIDIA supplement](https://iridia.ulb.ac.be/supp/IridiaSupp2019-009/) |
| Same group, AAMAS 2018 | "Managing Byzantine Robots via Blockchain Technology in a Swarm Robotics Collective Decision Making Scenario" | [AAMAS PDF](https://ifaamas.csc.liv.ac.uk/Proceedings/aamas2018/pdfs/p541.pdf) |

Not found: a deployed ledger-coordinated swarm outside simulation or lab work. Not searched: ROS 2 DDS-security, other robotic-blockchain groups, auction-based task allocation (CBBA etc.) — the likeliest sources of closer prior art.

## How a Maya2C version would differ (each backed by the repo)

- **Wire formats are checked:** MAVLink 2 (HEARTBEAT, GLOBAL_POSITION_INT) and ROS 2 CDR (Pose, PoseStamped) tested against pymavlink and rosbags (`hal/swarm/src/lib.rs:7-12`).
- **Plans committed, conflicts proven:** sealed-bid auctions and committed flight plans with provable pairwise conflicts (`lib.rs:8-9`, `hal/swarm/src/paths.rs:99-110`); the ledger records commitments, avoidance checked off chain.
- **Post-quantum signatures** on each commitment; radio budgets tight (11,165 bytes, `hal/radio-transport/src/lib.rs:5-8`).
- **Limits:** "nothing in the node calls this crate, and no robot has flown with it" (`hal/swarm/src/lib.rs:12-13`); tests use 100 simulated agents; no Byzantine-robot comparison; no dag-bft wiring.

## Legal and ethical gates

- Civil use only. Aviation law for any real flight (permissions, remote-ID, no BVLOS without approval). Not researched here.
- Radio rules for the control link (see `city-mesh`).
- No weapons, targeting or military coordination.
- Real flights need a test range, a safety case and a human override.

## What may be said publicly

- Accurate: "Maya2C includes a research crate for committed flight plans and sealed task auctions, tested in simulation."
- **forbidden**: "first blockchain for robot swarms", "only swarm ledger", "unprecedented", "autonomous fleet network".

**Verdict:** RESEARCH. Promotion would need a Byzantine-agent simulation against published baselines, hardware-in-the-loop MAVLink tests, and an aviation-law review.

