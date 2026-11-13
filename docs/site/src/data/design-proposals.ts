// Proposed designs ADR-102..ADR-134 (ADR-116 is not among these PRs). Every entry is a
// written proposal on an open pull request, not a feature. Statuses and summaries
// were read from each ADR and its prior-art file on the PR branches.
export type Category = "wallet" | "tokens" | "privacy" | "scaling" | "interop" | "AI" | "economics";
export type DesignStatus = "proposed" | "options";

export interface DesignProposal {
  id: number;
  title: string;
  /** One plain line: what it would give you. */
  gives: string;
  category: Category;
  status: DesignStatus;
  pr: number;
  needsDecision: boolean;
  /** Facilities elsewhere that do something similar, in neutral words. */
  comparable: string;
  /** What would change for a person using the network. */
  changes: string;
  /** What the document leaves open. */
  open: string;
}

const DARK = "Activation is left for a later, separate decision; the feature stays switched off until then.";
const P = { a: 465, b: 468, c: 469, d: 470, e: 472 } as const;

export const categories: { key: Category; label: string }[] = [
  { key: "wallet", label: "Wallet" },
  { key: "tokens", label: "Tokens" },
  { key: "privacy", label: "Privacy" },
  { key: "scaling", label: "Scaling" },
  { key: "interop", label: "Interop" },
  { key: "AI", label: "AI" },
  { key: "economics", label: "Economics" },
];

export const prUrl = (n: number) => `https://github.com/EricWijesinghe/Maya2C/pull/${n}`;

export const proposals: DesignProposal[] = [
  { id: 102, title: "Stealth addresses", gives: "Receive payments that nobody can link to your published address.", category: "privacy", status: "proposed", pr: P.a, needsDecision: false,
    comparable: "Monero one-time outputs, ERC-5564 and BIP-352 do this with elliptic-curve keys; this design uses a post-quantum key exchange instead.",
    changes: "Each payment to you would land on a fresh address that only you can recognise. Spending it still shows the spend address.", open: DARK },
  { id: 103, title: "Native fungible tokens", gives: "Issue your own token with a hard supply cap and an optional mint authority.", category: "tokens", status: "proposed", pr: P.a, needsDecision: false,
    comparable: "ERC-20 leaves minting and caps to each contract; Solana's token program enforces a mint authority in the protocol.",
    changes: "Token issuers could mint, burn or give up the mint right on chain, with the cap fixed at creation. Fees stay in the native coin.", open: DARK },
  { id: 104, title: "One-of-a-kind assets (NFTs)", gives: "Own a collectible by its content hash, with optional on-chain royalty rules.", category: "tokens", status: "proposed", pr: P.a, needsDecision: false,
    comparable: "ERC-721 and ERC-1155 point to off-chain data; ERC-2981 signals royalties without enforcing them.",
    changes: "Items would be 32-byte hashes inside a collection; images stay off chain. Royalties apply to sales settled on the chain.", open: DARK },
  { id: 105, title: "Name service", gives: "Pay name.maya instead of a long address.", category: "wallet", status: "proposed", pr: P.a, needsDecision: false,
    comparable: "ENS rents names with a grace period and a decaying premium after expiry; this design adds ASCII-only labels to avoid look-alikes.",
    changes: "Wallets could resolve short names. Names are renewed by block count and registered in two steps to resist sniping.", open: "Non-Latin names are left out of version one on purpose. " + DARK },
  { id: 106, title: "Sponsored fees", gives: "Let someone else pay the network fee for your transaction.", category: "wallet", status: "proposed", pr: P.a, needsDecision: false,
    comparable: "ERC-4337 paymasters and Sui sponsored transactions; Cosmos keeps a standing fee allowance in state.",
    changes: "A new user could act without holding any coin to begin with. The sponsor signs a ceiling and an expiry; the fee comes from the sponsor.", open: DARK },
  { id: 107, title: "Smart accounts with session keys", gives: "Give an app a limited key with spending limits instead of your main key.", category: "wallet", status: "proposed", pr: P.a, needsDecision: false,
    comparable: "ERC-4337 accounts and wallet modules offer session keys and limits as contract code.",
    changes: "Limits and expiry would be checked by the chain itself. Depends on the account-authority work in PR #447.", open: DARK },
  { id: 108, title: "Liquid staking receipt", gives: "Stake and still hold a transferable receipt for your stake.", category: "economics", status: "proposed", pr: P.a, needsDecision: false,
    comparable: "Lido uses a rebasing token and shares slashing across holders; Jito uses an exchange-rate token over a stake pool.",
    changes: "A protocol pool would spread deposits over the active validators and issue a receipt whose rate falls if the pool is slashed.", open: DARK },
  { id: 109, title: "Scheduled payments", gives: "Set up a payment that runs by itself at a block height.", category: "wallet", status: "proposed", pr: P.a, needsDecision: false,
    comparable: "Cosmos authz needs a relayer to submit each payment; this design runs due payments inside the block.",
    changes: "Subscriptions would not depend on a third party staying online. Schedules count blocks, not clock time.", open: "A calendar month is just an estimate in blocks. " + DARK },
  { id: 110, title: "Shielded user tokens", gives: "Send your own tokens privately in the existing shielded pool.", category: "privacy", status: "proposed", pr: P.b, needsDecision: false,
    comparable: "Zcash Shielded Assets add an asset id to notes; Solana confidential transfers hide amounts with an optional auditor key.",
    changes: "Notes would carry an asset id in one shared pool. Today's native-coin notes stay valid with no migration.", open: "The shielded circuit has not been audited, and shielded transfers are switched off at launch. " + DARK },
  { id: 111, title: "Treasury and grants", gives: "Fund projects through a vote, with grants that unlock over time.", category: "economics", status: "proposed", pr: P.b, needsDecision: false,
    comparable: "Cardano treasury withdrawals and Polkadot OpenGov spend tracks route spending through governance.",
    changes: "A treasury account with no key would be spent by a passed proposal alone, under a per-epoch ceiling.", open: DARK },
  { id: 112, title: "Pull oracle", gives: "Bring a signed price report with the transaction that needs it.", category: "interop", status: "proposed", pr: P.b, needsDecision: false,
    comparable: "Pyth and Chainlink Data Streams let the user submit a signed report for on-chain verification.",
    changes: "Apps could read fresh data on demand instead of waiting for a pushed round.", open: DARK },
  { id: 113, title: "Vote delegation", gives: "Hand your vote to someone you trust and take it back later.", category: "economics", status: "proposed", pr: P.b, needsDecision: false,
    comparable: "Cardano DReps and Polkadot conviction voting both support delegating vote weight.",
    changes: "One hop of delegation with a conviction you choose. Delegated stake unlocks after a lead time long enough to cover open proposals.", open: DARK },
  { id: 114, title: "Contract timers", gives: "Let a contract wake itself up at a later block.", category: "scaling", status: "proposed", pr: P.b, needsDecision: false,
    comparable: "Internet Computer canister timers and Substrate's scheduler run calls at a later time or block.",
    changes: "Contracts could do delayed work without an outside bot. Failures are harmless no-ops.", open: DARK },
  { id: 115, title: "Post-quantum randomness", gives: "A public random beacon that does not rely on elliptic curves.", category: "privacy", status: "proposed", pr: P.b, needsDecision: false,
    comparable: "Published lattice and hash-based verifiable random functions exist as papers; none is a settled standard.",
    changes: "A hash-chain scheme would feed the beacon, with its small withholding bias stated. A lattice scheme is reserved.", open: "The lattice scheme has no activation until a research gate is met." },
  { id: 117, title: "Multisig suite diversity", gives: "Require approvals from more than one signature family.", category: "wallet", status: "proposed", pr: P.c, needsDecision: false,
    comparable: "Algorand's roadmap describes multisig mixing classical, Falcon and hybrid keys.",
    changes: "A group account could demand that some approvals use a different algorithm, so one broken scheme cannot approve alone.", open: DARK },
  { id: 118, title: "Compressed state collections", gives: "Hold huge collections while storing one hash on chain.", category: "scaling", status: "proposed", pr: P.c, needsDecision: false,
    comparable: "Solana compressed NFTs keep a Merkle root on chain; Light Protocol generalises it with proofs.",
    changes: "Millions of items could cost a small fixed amount of chain state. Indexers rebuild the tree from the log.", open: DARK },
  { id: 119, title: "Data carrier", gives: "A bounded, plainly labelled way to write data into a block.", category: "scaling", status: "proposed", pr: P.c, needsDecision: false,
    comparable: "Bitcoin OP_RETURN, and Ordinals inscriptions that appeared when carriers were capped.",
    changes: "A tagged data transaction up to 64 KiB that changes no state. Indexers read the tag.", open: DARK },
  { id: 120, title: "Cross-chain token standard", gives: "Move a token across chains with a cap on what any bridge can mint.", category: "interop", status: "proposed", pr: P.c, needsDecision: false,
    comparable: "ERC-7281 (xERC20) and Chainlink CCT set per-bridge mint limits.",
    changes: "A bridge route could mint against a proven inbound event alone, within its own window limit.", open: "Depends on ADR-103 and ADR-122. " + DARK },
  { id: 121, title: "Blockspace lanes", gives: "Reserve a steady share of block space for your app.", category: "economics", status: "proposed", pr: P.c, needsDecision: false,
    comparable: "Ethereum execution tickets and preconfirmation auctions are research proposals for selling future block space.",
    changes: "Lanes would be won in an auction each epoch, with a ceiling so nobody can buy the whole chain.", open: DARK },
  { id: 122, title: "Cross-chain messaging (inbound)", gives: "Receive a verified message from an Ethereum contract.", category: "interop", status: "proposed", pr: P.c, needsDecision: false,
    comparable: "IBC channels give ordered, exactly-once delivery through light clients.",
    changes: "A message is accepted when a finalized Ethereum block that holds it is proven. Delivery is once per source nonce.", open: "Outbound messages are not part of this design. " + DARK },
  { id: 123, title: "Blob transactions", gives: "Carry large data outside the block with a proof it was available.", category: "scaling", status: "proposed", pr: P.c, needsDecision: false,
    comparable: "Ethereum EIP-4844 blobs live in a sidecar, expire after weeks, and have their own fee market.",
    changes: "Validators vote after holding the data. Bytes are deleted at expiry; the commitment stays.", open: DARK },
  { id: 124, title: "Inclusion and fair ordering", gives: "A valid transaction cannot be kept out, and authors cannot choose its place.", category: "privacy", status: "proposed", pr: P.c, needsDecision: false,
    comparable: "Ethereum inclusion-list proposals (FOCIL) and fair-ordering research.",
    changes: "Order within a batch would follow a hash, not the author's queue.", open: "Consensus-critical: needs two human reviewers, a spec update and conformance vectors before acceptance." },
  { id: 125, title: "Succinct chain-state proof", gives: "Check the whole chain's history from a small proof.", category: "interop", status: "proposed", pr: P.d, needsDecision: false,
    comparable: "Mina keeps a constant-size recursive proof of its chain.",
    changes: "Light clients could verify epoch handoffs without replaying them. The chain itself never verifies one.", open: "Recursion is a measured gate, not a promise. The early phases are off-chain tooling." },
  { id: 126, title: "Bitcoin checkpoint anchoring", gives: "A timestamp of the chain's state recorded in Bitcoin.", category: "interop", status: "proposed", pr: P.d, needsDecision: false,
    comparable: "Komodo dPoW and Babylon write checkpoints into Bitcoin via OP_RETURN.",
    changes: "A named operator posts a short hash; anyone can verify it. Consensus reads none of it.", open: "Off-chain tooling; no node or wire change." },
  { id: 127, title: "VRF host function", gives: "Let contracts verify randomness proofs and draw fairly.", category: "scaling", status: "proposed", pr: P.d, needsDecision: false,
    comparable: "Algorand's vrf_verify opcode and Aptos native randomness.",
    changes: "Contracts get one verification call and a safe commit-then-draw pattern.", open: DARK },
  { id: 128, title: "Threshold decryption for contracts", gives: "Contracts can hold secrets that open together at a set height.", category: "privacy", status: "proposed", pr: P.d, needsDecision: true,
    comparable: "Shutter keypers, Ferveo and Fairblock use committee decryption for private mempools.",
    changes: "A second committee would open sealed data after a bounded delay and hand it to the contract.", open: "This changes the protocol's trust model, so it needs an explicit owner decision before it moves forward." },
  { id: 129, title: "Verifiable compute jobs", gives: "Pay for compute and get receipts several workers agree on.", category: "AI", status: "proposed", pr: P.d, needsDecision: false,
    comparable: "EigenAI and Gensyn check inference by re-execution and refereed delegation.",
    changes: "Jobs, escrow and claims live on chain; models and weights stay off every node.", open: "Depends on the verifiable-compute design (ADR-079) and an oracle-rule decision. " + DARK },
  { id: 130, title: "Agent registry and payments", gives: "Register an AI agent and let it pay within strict limits.", category: "AI", status: "proposed", pr: P.d, needsDecision: false,
    comparable: "ERC-8004 registries and ERC-4337 session keys with spend caps.",
    changes: "An agent is a limited session key plus a registry entry. The chain enforces limits and reads no agent card.", open: "Depends on ADR-107. " + DARK },
  { id: 131, title: "State rent or storage deposit", gives: "Choose how the chain prices the state each record occupies.", category: "economics", status: "options", pr: P.e, needsDecision: true,
    comparable: "Solana requires a rent-exempt minimum; NEAR locks tokens per byte stored; Ethereum has state-expiry proposals.",
    changes: "Nothing yet. Each option would change how long records live and who pays to keep them.", open: "No option is chosen. Every option would be a switched-off feature that does not block launch." },
  { id: 132, title: "Shared security and restaking", gives: "Let Maya2C stake secure something beyond Maya2C.", category: "economics", status: "options", pr: P.e, needsDecision: true,
    comparable: "EigenLayer, Symbiotic and Babylon let collateral back several services at once.",
    changes: "Nothing yet. Options range from not doing it to a full export of security.", open: "No option is chosen. All but option A fall outside the current launch scope." },
  { id: 133, title: "Paid storage market", gives: "Pay providers to keep data and prove they still hold it.", category: "scaling", status: "options", pr: P.e, needsDecision: true,
    comparable: "Filecoin proves possession on a schedule; Arweave pays once into an endowment.",
    changes: "Nothing yet. Options build on blobs and the archive.", open: "No option is chosen. All but option A depend on blob transactions (ADR-123)." },
  { id: 134, title: "Appchains on Maya2C validators", gives: "Run your own chain secured by Maya2C's validator set.", category: "interop", status: "options", pr: P.e, needsDecision: true,
    comparable: "Avalanche L1s keep their own validators with a continuous fee; Polkadot assigns relay validators to parachains.",
    changes: "Nothing yet. Options run from none to a replicated-security framework.", open: "No option is chosen. All but option A fall outside the current launch scope." },
];
