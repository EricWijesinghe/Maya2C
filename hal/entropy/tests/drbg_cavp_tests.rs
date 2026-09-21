//! NIST CAVP known answers for `HMAC_DRBG` / SHA-256 (SP 800-90A).
//!
//! Vectors: `tests/vectors/hmac_drbg_sha256_{no_reseed,pr_false}.rsp`, the
//! `[SHA-256]` sections of `drbgtestvectors.zip` (provenance in each file's
//! header). The CAVP procedure is: instantiate; (reseed, in the `pr_false`
//! file); generate and discard; generate and compare.

use maya_entropy::drbg::HmacDrbg;

struct Case {
    entropy: Vec<u8>,
    nonce: Vec<u8>,
    personalization: Vec<u8>,
    entropy_reseed: Vec<u8>,
    additional_reseed: Vec<u8>,
    additional: Vec<Vec<u8>>,
    returned: Vec<u8>,
}

fn parse(text: &str) -> Vec<Case> {
    let mut cases = Vec::new();
    let mut current: Option<Case> = None;
    // Section headers (`[...]`), comments, `**` step markers and the indented
    // intermediate `V` / `Key` values also contain " = "; only the top-level
    // fields are inputs and answers.
    let field_line = |l: &&str| l.starts_with(|c: char| c.is_ascii_alphabetic());
    for line in text.lines().filter(field_line) {
        let Some((key, value)) = line.split_once(" = ") else {
            continue;
        };
        let bytes = || hex::decode(value.trim()).expect("hex");
        match key.trim() {
            "COUNT" => {
                if let Some(case) = current.take() {
                    cases.push(case);
                }
                current = Some(Case {
                    entropy: Vec::new(),
                    nonce: Vec::new(),
                    personalization: Vec::new(),
                    entropy_reseed: Vec::new(),
                    additional_reseed: Vec::new(),
                    additional: Vec::new(),
                    returned: Vec::new(),
                });
            }
            field => {
                let case = current.as_mut().expect("COUNT first");
                match field {
                    "EntropyInput" => case.entropy = bytes(),
                    "Nonce" => case.nonce = bytes(),
                    "PersonalizationString" => case.personalization = bytes(),
                    "EntropyInputReseed" => case.entropy_reseed = bytes(),
                    "AdditionalInputReseed" => case.additional_reseed = bytes(),
                    "AdditionalInput" => case.additional.push(bytes()),
                    "ReturnedBits" => case.returned = bytes(),
                    other => panic!("unexpected field {other}"),
                }
            }
        }
    }
    cases.extend(current);
    cases
}

fn run(file: &str, reseed: bool) -> usize {
    let path = format!("{}/tests/vectors/{file}", env!("CARGO_MANIFEST_DIR"));
    let cases = parse(&std::fs::read_to_string(&path).expect("rsp"));
    for (i, case) in cases.iter().enumerate() {
        let mut drbg = HmacDrbg::instantiate(&case.entropy, &case.nonce, &case.personalization)
            .expect("instantiate");
        if reseed {
            drbg.reseed(&case.entropy_reseed, &case.additional_reseed)
                .expect("reseed");
        }
        let mut out = vec![0u8; case.returned.len()];
        drbg.generate(&mut out, &case.additional[0])
            .expect("first generate");
        drbg.generate(&mut out, &case.additional[1])
            .expect("second generate");
        assert_eq!(out, case.returned, "{file} case {i}");
    }
    cases.len()
}

#[test]
fn hmac_drbg_sha256_matches_cavp_without_reseed() {
    assert_eq!(run("hmac_drbg_sha256_no_reseed.rsp", false), 240);
}

#[test]
fn hmac_drbg_sha256_matches_cavp_with_reseed() {
    assert_eq!(run("hmac_drbg_sha256_pr_false.rsp", true), 240);
}

#[test]
fn short_entropy_and_oversized_requests_are_refused() {
    assert!(HmacDrbg::instantiate(&[0; 31], &[0; 16], &[]).is_err());
    let mut drbg = HmacDrbg::instantiate(&[1; 32], &[2; 16], &[]).expect("instantiate");
    assert!(drbg.reseed(&[0; 31], &[]).is_err());
    let mut big = vec![0u8; maya_entropy::drbg::MAX_REQUEST_BYTES + 1];
    assert!(drbg.generate(&mut big, &[]).is_err());
    assert!(format!("{drbg:?}").contains("redacted"));
}
