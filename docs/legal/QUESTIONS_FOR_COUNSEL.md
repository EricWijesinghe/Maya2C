# Questions for counsel

Master Prompt 18 §7. **These are questions, not answers.** Nothing in this
repository is legal advice (docs/LEGAL_NOTICE.md); each item below needs a
qualified lawyer in the relevant jurisdiction.

## Token classification

1. **EU (MiCA, Regulation (EU) 2023/1114):** is the native token a
   "crypto-asset other than an asset-referenced token or e-money token"
   (Title II)? If a white paper is required, who is the offeror at genesis?
2. Does staking-reward issuance or fee burning change that analysis?
3. **United States:** under the *Howey* test, what facts about the genesis
   allocation, any sale, and the foundation's role would matter? Does the
   answer differ between the token at genesis and after decentralisation of
   the admin keys (Master Prompt 9's burn ceremony)?
4. Are there other target jurisdictions (UK, Singapore, Switzerland, Japan,
   UAE) whose regimes change the design or the launch sequence?

## Structure

5. Which legal entity, if any, should hold the treasury multisig keys before
   governance controls them, and what fiduciary duties follow?
6. Validators: does running a validator, or accepting delegation, create
   licensing obligations (e.g. as a custodian or staking-service provider) in
   the target jurisdictions?
7. Can grants from the on-chain treasury (Master Prompt 18 §6) be made to
   individuals, and what tax reporting follows?

## Sanctions and front ends

8. What sanctions screening is expected of the reference wallet, the
   explorer, the faucet and the public RPC gateway — each operated by whom?
9. Is screening at the front end sufficient, or is there an expectation at
   the protocol level (which a permissionless chain cannot implement without
   a censoring authority)?
10. Travel rule (FATF Recommendation 16; EU Transfer of Funds Regulation
    2023/1113): which message formats should the exchange integration kit
    support, and are there obligations on self-hosted wallets?

## Data protection

11. The identity module stores commitments and roots only, never claims or
    biometrics (docs/identity.md). Does a commitment to personal data count
    as personal data under GDPR when the chain is immutable?
12. The telemetry collector and faucet log IP addresses. What retention and
    notice obligations apply?

## Launch economics

13. Any incentivised-testnet rewards or airdrop: securities, tax and
    consumer-protection implications per jurisdiction.
14. Vesting and clawback clauses in allocation contracts: enforceability.
