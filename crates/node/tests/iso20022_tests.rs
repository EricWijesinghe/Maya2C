//! The ISO 20022 bridge, end to end: a bank message in, a block, a statement
//! out, and the statement back in again.
//!
//! ## What "bidirectional" means here, and what it does not
//!
//! It does **not** mean byte-identical XML. XML has no canonical form — a
//! namespace prefix, whitespace between elements and the order of optional
//! siblings are all a serializer's choice, and every counterparty makes
//! different ones. A test that asserted byte equality would be asserting that
//! this crate and its own writer agree, which is true and worth nothing.
//!
//! So round-trip equality is asserted on the **parsed model**: the value a
//! document decodes to must survive rendering and reparsing unchanged. That is
//! the property a bank actually needs, because it is the one that says a
//! payment instruction means the same thing after it has been forwarded.
//!
//! ## The statement is drawn from a block that verified
//!
//! `apply_block_checked` refuses a block whose declared state root execution
//! does not reproduce (invariant 24), so the statement in
//! `a_statement_is_drawn_from_a_block_that_verified` is rendered from state
//! that passed that check rather than from a block that merely parsed.

use custom_l1_node::core::{Block, BlockHeader, Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, generate_signing_key};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::iso20022_bridge::{
    Settled, StatementSource, address_for_account, transaction_for,
};
use custom_l1_node::state::{Account, Address, BlockContext, StateDB};

use maya_iso20022::amount::Amount;
use maya_iso20022::bridge::{self, Origin, PaymentIntent};
use maya_iso20022::camt053::{self, Direction, Entry, Statement};
use maya_iso20022::party::{AccountId, Bic, Currency, Iban, Party};
use maya_iso20022::{Error, pacs008, pacs009, sanctions, xml};

use maya_zk_privacy::sanctions::{SanctionsList, prove, verify};

use tempfile::TempDir;

// ---------------------------------------------------------------------------
// fixtures
// ---------------------------------------------------------------------------

/// The chain this bridge is allowed on.
const CHAIN: &str = "l1-testnet-1";

/// Published specimen IBANs: real check digits, no real account behind them.
const DEBTOR_IBAN: &str = "GB82WEST12345698765432";
const CREDITOR_IBAN: &str = "DE89370400440532013000";

fn pacs008_xml(amount: &str, currency: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns="urn:iso:std:iso:20022:tech:xsd:pacs.008.001.08">
  <FIToFICstmrCdtTrf>
    <GrpHdr>
      <MsgId>MSG-0001</MsgId>
      <CreDtTm>2026-09-13T09:00:00Z</CreDtTm>
      <NbOfTxs>1</NbOfTxs>
    </GrpHdr>
    <CdtTrfTxInf>
      <PmtId>
        <InstrId>INSTR-0001</InstrId>
        <EndToEndId>E2E-0001</EndToEndId>
      </PmtId>
      <IntrBkSttlmAmt Ccy="{currency}">{amount}</IntrBkSttlmAmt>
      <Dbtr><Nm>Acme Ltd</Nm></Dbtr>
      <DbtrAcct><Id><IBAN>{DEBTOR_IBAN}</IBAN></Id></DbtrAcct>
      <DbtrAgt><FinInstnId><BICFI>DEUTDEFF</BICFI></FinInstnId></DbtrAgt>
      <Cdtr><Nm>Beta GmbH</Nm></Cdtr>
      <CdtrAcct><Id><IBAN>{CREDITOR_IBAN}</IBAN></Id></CdtrAcct>
      <CdtrAgt><FinInstnId><BICFI>BNPAFRPP</BICFI></FinInstnId></CdtrAgt>
      <RmtInf><Ustrd>Invoice 4711</Ustrd></RmtInf>
    </CdtTrfTxInf>
  </FIToFICstmrCdtTrf>
</Document>"#
    )
}

fn pacs009_xml() -> String {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns="urn:iso:std:iso:20022:tech:xsd:pacs.009.001.08">
  <FinInstnCdtTrf>
    <GrpHdr>
      <MsgId>MSG-9001</MsgId>
      <CreDtTm>2026-09-13T09:05:00Z</CreDtTm>
      <NbOfTxs>1</NbOfTxs>
    </GrpHdr>
    <CdtTrfTxInf>
      <PmtId><EndToEndId>E2E-9001</EndToEndId></PmtId>
      <IntrBkSttlmAmt Ccy="EUR">50000.00</IntrBkSttlmAmt>
      <Dbtr><Nm>Deutsche Bank AG</Nm></Dbtr>
      <DbtrAcct><Id><Othr><Id>NOSTRO-DE-01</Id></Othr></Id></DbtrAcct>
      <DbtrAgt><FinInstnId><BICFI>DEUTDEFF</BICFI></FinInstnId></DbtrAgt>
      <Cdtr><Nm>BNP Paribas</Nm></Cdtr>
      <CdtrAcct><Id><Othr><Id>NOSTRO-FR-01</Id></Othr></Id></CdtrAcct>
      <CdtrAgt><FinInstnId><BICFI>BNPAFRPP</BICFI></FinInstnId></CdtrAgt>
    </CdtTrfTxInf>
  </FinInstnCdtTrf>
</Document>"#
        .to_owned()
}

fn iban(text: &str) -> AccountId {
    AccountId::Iban(Iban::parse(text).expect("specimen IBAN"))
}

fn eur() -> Currency {
    Currency::parse("EUR").expect("EUR")
}

struct Fixture {
    db: StateDB,
    _dir: TempDir,
}

fn fixture(funded: &[(Address, u64)]) -> Fixture {
    let dir = TempDir::new().expect("temp dir");
    let db = StateDB::open(dir.path()).expect("open");
    for (address, balance) in funded {
        db.put_account(
            address,
            &Account {
                balance: *balance,
                nonce: 0,
            },
        )
        .expect("fund");
    }
    Fixture { db, _dir: dir }
}

fn block_of(transactions: Vec<Transaction>) -> Block {
    Block::new(
        BlockHeader {
            prev_hash: [0u8; 32],
            state_root: [0u8; 32],
            timestamp: 1_789_000_000,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(0),
            tx_root: [0; 32],
        },
        transactions,
    )
}

fn signed(mut transaction: Transaction, key: &HybridSigningKey) -> Transaction {
    transaction.sign(key).expect("sign");
    transaction
}

// ---------------------------------------------------------------------------
// 1. parsing, and the round trip that actually matters
// ---------------------------------------------------------------------------

#[test]
fn a_pacs008_parses_into_the_fields_the_bridge_acts_on() {
    let message = pacs008::parse(pacs008_xml("1234.56", "EUR").as_bytes()).expect("parse");
    assert_eq!(message.message_id, "MSG-0001");
    assert_eq!(message.transactions.len(), 1);

    let transfer = &message.transactions[0];
    assert_eq!(transfer.end_to_end_id, "E2E-0001");
    assert_eq!(transfer.instruction_id.as_deref(), Some("INSTR-0001"));
    // 1234.56 EUR is 123_456 base units, not 1_234 and not 123_456_00.
    assert_eq!(transfer.amount.base_units(), 123_456);
    assert_eq!(transfer.debtor.account.as_str(), DEBTOR_IBAN);
    assert_eq!(transfer.creditor.account.as_str(), CREDITOR_IBAN);
    assert_eq!(transfer.remittance.as_deref(), Some("Invoice 4711"));
}

#[test]
fn every_message_survives_a_render_and_reparse() {
    // The bidirectional property, asserted on the model rather than the bytes.
    let pacs008 = pacs008::parse(pacs008_xml("1234.56", "EUR").as_bytes()).expect("parse");
    let reparsed = pacs008::parse(pacs008.to_xml().expect("render").as_bytes()).expect("reparse");
    assert_eq!(reparsed, pacs008);

    let pacs009 = pacs009::parse(pacs009_xml().as_bytes()).expect("parse");
    let reparsed = pacs009::parse(pacs009.to_xml().expect("render").as_bytes()).expect("reparse");
    assert_eq!(reparsed, pacs009);
}

#[test]
fn a_namespace_prefix_does_not_change_what_a_message_means() {
    // Every counterparty picks its own prefix, and a bridge that only read
    // unprefixed documents would reject most real traffic.
    let plain = pacs008_xml("1234.56", "EUR");
    let prefixed = plain
        .replace("<Document xmlns=", "<ns0:Document xmlns:ns0=")
        .replace("</Document>", "</ns0:Document>");
    assert_eq!(
        pacs008::parse(prefixed.as_bytes()).expect("prefixed"),
        pacs008::parse(plain.as_bytes()).expect("plain"),
    );
}

#[test]
fn a_pacs009_carrying_remittance_information_is_refused() {
    let message = pacs009_xml().replace(
        "</CdtTrfTxInf>",
        "<RmtInf><Ustrd>not a field of this message</Ustrd></RmtInf></CdtTrfTxInf>",
    );
    assert!(matches!(
        pacs009::parse(message.as_bytes()),
        Err(Error::Unexpected { .. })
    ));
}

#[test]
fn a_declared_transaction_count_that_disagrees_is_refused() {
    // Refused rather than reconciled to either number: the two readings settle
    // different amounts of money.
    let message =
        pacs008_xml("1234.56", "EUR").replace("<NbOfTxs>1</NbOfTxs>", "<NbOfTxs>2</NbOfTxs>");
    assert!(matches!(
        pacs008::parse(message.as_bytes()),
        Err(Error::Invalid {
            field: "NbOfTxs",
            ..
        })
    ));
}

// ---------------------------------------------------------------------------
// 2. the hostile-input properties
// ---------------------------------------------------------------------------

#[test]
fn a_doctype_is_refused_before_anything_is_read() {
    // The billion-laughs shape. It is refused for carrying a DOCTYPE at all,
    // which is why no expansion limit has to be tuned.
    let bomb = r#"<?xml version="1.0"?>
<!DOCTYPE lolz [
  <!ENTITY lol "lol">
  <!ENTITY lol2 "&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;">
  <!ENTITY lol3 "&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;">
]>
<Document><FIToFICstmrCdtTrf>&lol3;</FIToFICstmrCdtTrf></Document>"#;
    let error = xml::parse(bomb.as_bytes()).expect_err("doctype");
    assert!(error.to_string().contains("DOCTYPE"), "{error}");
    assert!(pacs008::parse(bomb.as_bytes()).is_err());
}

#[test]
fn an_external_entity_reference_never_resolves_to_a_value() {
    // The XXE shape. There is no resolver to point at a file, and the
    // undeclared reference fails unescaping rather than reading as empty —
    // which is the reading that would let a field slip past a check.
    let xxe = r#"<?xml version="1.0"?>
<Document><FIToFICstmrCdtTrf><GrpHdr><MsgId>&xxe;</MsgId></GrpHdr></FIToFICstmrCdtTrf></Document>"#;
    assert!(xml::parse(xxe.as_bytes()).is_err());
}

#[test]
fn a_deeply_nested_document_is_refused_rather_than_overflowing_the_stack() {
    let depth = xml::MAX_DEPTH + 10;
    let mut bomb = String::from("<Document>");
    for _ in 0..depth {
        bomb.push_str("<a>");
    }
    for _ in 0..depth {
        bomb.push_str("</a>");
    }
    bomb.push_str("</Document>");
    assert!(matches!(
        xml::parse(bomb.as_bytes()),
        Err(Error::Bound {
            what: "element nesting",
            ..
        })
    ));
}

#[test]
fn a_document_past_the_size_bound_is_refused_before_it_is_parsed() {
    let huge = vec![b'x'; xml::MAX_DOCUMENT_BYTES + 1];
    assert!(matches!(
        xml::parse(&huge),
        Err(Error::Bound {
            what: "document",
            ..
        })
    ));
}

#[test]
fn a_sub_unit_amount_is_refused_rather_than_rounded() {
    // The one that would otherwise be a payment short by a cent, discovered at
    // reconciliation rather than at the door.
    let error = pacs008::parse(pacs008_xml("1234.567", "EUR").as_bytes()).expect_err("3 places");
    assert!(error.to_string().contains("will not round"), "{error}");
}

// ---------------------------------------------------------------------------
// 3. the bridge, and the chain it refuses
// ---------------------------------------------------------------------------

#[test]
fn the_bridge_refuses_a_chain_that_holds_value() {
    // The sealed envelope's confidentiality is classical and the envelope is on
    // chain forever, so a debtor's account number sealed today is readable when
    // that assumption fails. Not a flag to flip when the bridge is "ready".
    let message = pacs008::parse(pacs008_xml("1234.56", "EUR").as_bytes()).expect("parse");
    assert!(bridge::intents_from_pacs008(&message, "mainnet").is_err());
    assert!(bridge::intents_from_pacs008(&message, CHAIN).is_ok());
}

#[test]
fn a_message_becomes_one_intent_per_transaction() {
    let message = pacs008::parse(pacs008_xml("1234.56", "EUR").as_bytes()).expect("parse");
    let intents = bridge::intents_from_pacs008(&message, CHAIN).expect("intents");
    assert_eq!(intents.len(), 1);
    assert_eq!(intents[0].origin, Origin::CustomerCreditTransfer);
    assert_eq!(intents[0].amount.base_units(), 123_456);
}

#[test]
fn a_bank_account_maps_to_the_same_address_every_time() {
    // Derived, never stored: a table is state two nodes can disagree about and
    // a thing an operator can edit to redirect a payment.
    let account = iban(DEBTOR_IBAN);
    assert_eq!(address_for_account(&account), address_for_account(&account));
    assert_ne!(
        address_for_account(&account),
        address_for_account(&iban(CREDITOR_IBAN))
    );
}

#[test]
fn a_payment_the_virtual_account_cannot_cover_is_refused_at_the_gateway() {
    let message = pacs008::parse(pacs008_xml("1234.56", "EUR").as_bytes()).expect("parse");
    let intent = bridge::intents_from_pacs008(&message, CHAIN).expect("intents")[0].clone();

    let debtor = address_for_account(&intent.debtor.account);
    let short = fixture(&[(debtor, 1_000)]);
    assert!(transaction_for(&short.db, &intent, 0).is_err());

    let funded = fixture(&[(debtor, 200_000)]);
    assert!(transaction_for(&funded.db, &intent, 0).is_ok());
}

// ---------------------------------------------------------------------------
// 4. a statement drawn from a block that verified
// ---------------------------------------------------------------------------

#[test]
fn a_statement_is_drawn_from_a_block_that_verified() {
    let message = pacs008::parse(pacs008_xml("1234.56", "EUR").as_bytes()).expect("parse");
    let intent = bridge::intents_from_pacs008(&message, CHAIN).expect("intents")[0].clone();

    // The debtor's virtual account is funded, and the gateway signs for it.
    // In production the gateway's key is what authorises the debit; here it is
    // a key generated for the test, and what is being checked is the statement,
    // not the custody model.
    let gateway = generate_signing_key().expect("keygen");
    let sender = gateway.address();
    let creditor = address_for_account(&intent.creditor.account);

    let fixture = fixture(&[(sender, 1_000_000)]);
    let db = &fixture.db;

    let payment = signed(
        Transaction::new(
            Vec::new(),
            vec![TxOutput {
                recipient: creditor,
                amount: intent.amount.base_units(),
            }],
            0,
        ),
        &gateway,
    );
    let block = block_of(vec![payment]);

    // The apply path that checks the declared state root (invariant 24). The
    // statement below is therefore drawn from state that verified, not from a
    // block that merely decoded.
    let root = db
        .preview_root(&block, BlockContext::at_height(1))
        .expect("preview");
    let mut checked = block.clone();
    checked.header.state_root = root;
    checked.header.tx_root = block.header.tx_root;
    db.apply_block_checked(&checked, BlockContext::at_height(1))
        .expect("the block's declared state root is the one execution produced");

    assert_eq!(db.get_account(&creditor).expect("account").balance, 123_456);

    let statement = StatementSource::for_block(
        "STMT-MSG-1",
        "STMT-1",
        "2026-09-13T23:59:59Z",
        intent.creditor.account.clone(),
        eur(),
        Amount::from_base_units(0),
        &checked,
    )
    .expect("statement");

    // One credit, and a closing balance the entries produce.
    assert_eq!(statement.entries.len(), 1);
    assert_eq!(statement.entries[0].direction, Direction::Credit);
    assert_eq!(statement.entries[0].amount.base_units(), 123_456);
    assert_eq!(statement.closing.base_units(), 123_456);

    // And it agrees with the ledger it was drawn from.
    assert_eq!(
        statement.closing.base_units(),
        db.get_account(&creditor).expect("account").balance
    );

    // Out to XML and back: the direction a counterparty reads.
    let rendered = statement.to_xml().expect("render");
    assert!(rendered.contains("camt.053.001.08"));
    assert!(rendered.contains("1234.56"), "{rendered}");
    let reparsed = camt053::parse(rendered.as_bytes()).expect("reparse");
    assert_eq!(reparsed, statement);

    // `NtryRef` is Max35Text and a hex txid is 64 characters, so the reference
    // is a 16-byte prefix. Pinned here because the failure mode is a statement
    // this bridge writes and its own reader — and every counterparty's —
    // refuses. That happened; this is what catches it next time.
    let reference = statement.entries[0]
        .reference
        .as_deref()
        .expect("every settled entry carries one");
    assert_eq!(
        reference.len(),
        custom_l1_node::iso20022_bridge::ENTRY_REFERENCE_BYTES * 2
    );
    assert!(reference.len() <= 35, "NtryRef is Max35Text");
    assert!(
        hex::encode(&Settled::in_block(&checked, &creditor)[0].txid[..]).starts_with(reference),
        "the reference must be a prefix of the transaction id it names"
    );
}

#[test]
fn a_statement_whose_balances_do_not_follow_from_its_entries_is_refused() {
    // The camt.053 analogue of the chain's own value conservation: a total that
    // must equal the sum of what moved, checked rather than assumed.
    let statement = Statement::seal(
        "STMT-MSG-2",
        "STMT-2",
        "2026-09-13T23:59:59Z",
        iban(CREDITOR_IBAN),
        eur(),
        Amount::from_base_units(10_000),
        vec![Entry {
            reference: None,
            amount: Amount::from_base_units(2_500),
            direction: Direction::Credit,
        }],
    )
    .expect("seal");
    assert_eq!(statement.closing.base_units(), 12_500);

    let tampered = statement.to_xml().expect("render").replace(
        r#"<Amt Ccy="EUR">125.00</Amt><CdtDbtInd>CRDT</CdtDbtInd></Bal>"#,
        r#"<Amt Ccy="EUR">999.00</Amt><CdtDbtInd>CRDT</CdtDbtInd></Bal>"#,
    );
    let error = camt053::parse(tampered.as_bytes()).expect_err("tampered closing balance");
    assert!(error.to_string().contains("entries produce"), "{error}");
}

#[test]
fn a_statement_cannot_be_sealed_with_entries_that_overdraw_it() {
    let error = Statement::seal(
        "STMT-MSG-3",
        "STMT-3",
        "2026-09-13T23:59:59Z",
        iban(CREDITOR_IBAN),
        eur(),
        Amount::from_base_units(100),
        vec![Entry {
            reference: None,
            amount: Amount::from_base_units(500),
            direction: Direction::Debit,
        }],
    )
    .expect_err("overdrawn");
    assert!(error.to_string().contains("unsigned"), "{error}");
}

// ---------------------------------------------------------------------------
// 5. compliance without identity
// ---------------------------------------------------------------------------

#[test]
fn a_party_not_on_the_list_clears_it_without_revealing_who_they_are() {
    let message = pacs008::parse(pacs008_xml("1234.56", "EUR").as_bytes()).expect("parse");
    let intent = bridge::intents_from_pacs008(&message, CHAIN).expect("intents")[0].clone();

    // A list holding somebody else entirely.
    let listed = sanctions::identifier_for_account(&AccountId::other("BLOCKED-1").expect("id"));
    let list = SanctionsList::build([listed]).expect("list");
    let root = list.root().expect("root");

    for identifier in sanctions::identifiers_for_payment(&intent) {
        let witness = list
            .absence_witness(&identifier)
            .expect("this party is not listed");
        let proof = prove(&witness, root).expect("prove");
        assert!(verify(&proof, root).expect("verify"));
    }
}

#[test]
fn a_listed_party_cannot_produce_a_proof() {
    let message = pacs008::parse(pacs008_xml("1234.56", "EUR").as_bytes()).expect("parse");
    let intent = bridge::intents_from_pacs008(&message, CHAIN).expect("intents")[0].clone();

    // This time the debtor is on it.
    let debtor = sanctions::identifier_for_account(&intent.debtor.account);
    let list = SanctionsList::build([debtor]).expect("list");

    assert!(
        list.absence_witness(&debtor).is_err(),
        "a sanctioned party must not be able to build the prover's input at all"
    );

    // The creditor still clears, so the refusal is about the party and not
    // about the list being unusable.
    let creditor = sanctions::identifier_for_account(&intent.creditor.account);
    let root = list.root().expect("root");
    let witness = list.absence_witness(&creditor).expect("creditor is clear");
    assert!(verify(&prove(&witness, root).expect("prove"), root).expect("verify"));
}

#[test]
fn both_ends_and_both_agents_are_checked() {
    // A bridge that checked only the creditor would let a sanctioned debtor pay
    // anyone, which is the direction sanctions are usually written to stop.
    let debtor = Party::new(iban(DEBTOR_IBAN)).with_agent(Bic::parse("DEUTDEFF").expect("bic"));
    let creditor = Party::new(iban(CREDITOR_IBAN)).with_agent(Bic::parse("BNPAFRPP").expect("bic"));
    let intent = PaymentIntent {
        origin: Origin::CustomerCreditTransfer,
        message_id: "MSG-1".into(),
        end_to_end_id: "E2E-1".into(),
        debtor,
        creditor,
        amount: Amount::from_base_units(123_456),
        currency: eur(),
        remittance: None,
    };
    assert_eq!(sanctions::identifiers_for_payment(&intent).len(), 4);
}
