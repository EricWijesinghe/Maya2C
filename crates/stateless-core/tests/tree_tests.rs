//! The sparse tree, its witnesses, and the canonical encoding, over both
//! backends.

use maya_stateless_core::params::{
    KEY_BYTES, MAX_WITNESS_BYTES, TAG_INTERNAL, TAG_LEAF, TAG_OPAQUE,
};
use maya_stateless_core::{
    Blake3, Compress, Defect, Key, Node, PartialTree, RingSis, Value, VerifiedTree, sparse,
};

fn key(index: u64) -> Key {
    *blake3::hash(&index.to_le_bytes()).as_bytes()
}

fn value(index: u64) -> Value {
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&index.to_le_bytes());
    out
}

fn leaves(count: u64) -> Vec<(Key, Value)> {
    let mut set: Vec<_> = (0..count).map(|index| (key(index), value(index))).collect();
    set.sort_unstable_by_key(|leaf| leaf.0);
    set
}

fn verified<C: Compress>(
    compress: &C,
    set: &[(Key, Value)],
    touched: &[Key],
) -> VerifiedTree<C::Digest> {
    let root = sparse::root(compress, set).expect("root");
    sparse::open(compress, set, touched)
        .expect("open")
        .verify(compress, |digest| *digest == root)
        .expect("verify")
}

fn encoded<C: Compress>(tree: &PartialTree<C::Digest>) -> Vec<u8> {
    let mut out = Vec::new();
    tree.encode::<C>(&mut out);
    out
}

#[test]
fn an_empty_tree_is_the_empty_digest_and_one_leaf_is_its_leaf() {
    assert_eq!(sparse::root(&Blake3, &[]).expect("root"), Blake3.empty());
    let one = [(key(1), value(1))];
    assert_eq!(
        sparse::root(&Blake3, &one).expect("root"),
        Blake3.leaf(&key(1), &value(1))
    );
}

#[test]
fn unsorted_or_repeated_leaves_are_refused() {
    let mut set = leaves(4);
    set.swap(0, 1);
    assert_eq!(sparse::root(&Blake3, &set), Err(Defect::Unsorted));
    let repeated = [(key(1), value(1)), (key(1), value(2))];
    assert_eq!(sparse::root(&Blake3, &repeated), Err(Defect::Unsorted));
}

#[test]
fn a_witness_reads_present_and_absent_keys_and_nothing_else() {
    let set = leaves(256);
    let absent = key(10_000);
    let tree = verified(&Blake3, &set, &[set[7].0, absent]);

    assert_eq!(tree.get(&set[7].0).expect("opened"), Some(set[7].1));
    assert_eq!(tree.get(&absent).expect("opened"), None);
    let untouched = set
        .iter()
        .find(|leaf| leaf.0[0] != set[7].0[0])
        .expect("other");
    assert_eq!(
        tree.get(&untouched.0),
        Err(Defect::Unopened { key: untouched.0 })
    );
}

#[test]
fn writes_through_a_witness_reproduce_the_rebuilt_root() {
    let set = leaves(512);
    let fresh = [key(9_001), key(9_002), key(9_003)];
    let changed = [set[3].0, set[300].0];
    let touched: Vec<Key> = fresh.iter().chain(&changed).copied().collect();
    let mut tree = verified(&Blake3, &set, &touched);

    let mut expected = set.clone();
    for (index, target) in touched.iter().enumerate() {
        let new = value(77 + index as u64);
        tree.set(*target, new).expect("set");
        match expected.binary_search_by(|leaf| leaf.0.cmp(target)) {
            Ok(position) => expected[position].1 = new,
            Err(position) => expected.insert(position, (*target, new)),
        }
    }
    assert_eq!(
        tree.digest(&Blake3),
        sparse::root(&Blake3, &expected).expect("root")
    );
}

#[test]
fn keys_differing_only_in_the_last_bit_split_at_the_bottom() {
    let low: Key = [0; KEY_BYTES];
    let mut high = low;
    high[KEY_BYTES - 1] = 1;
    let mut tree = verified(&Blake3, &[], &[low, high]);
    tree.set(high, value(1)).expect("set");
    tree.set(low, value(2)).expect("set");
    let expected = [(low, value(2)), (high, value(1))];
    assert_eq!(
        tree.digest(&Blake3),
        sparse::root(&Blake3, &expected).expect("root")
    );
}

#[test]
fn a_witness_for_another_root_is_refused() {
    let set = leaves(32);
    let tree = sparse::open(&Blake3, &set, &[set[0].0]).expect("open");
    let other = sparse::root(&Blake3, &leaves(33)).expect("root");
    assert_eq!(
        tree.verify(&Blake3, |digest| *digest == other).map(|_| ()),
        Err(Defect::RootMismatch)
    );
}

#[test]
fn a_witness_round_trips_through_its_one_encoding() {
    let set = leaves(1_000);
    let tree = sparse::open(&Blake3, &set, &[set[1].0, key(5_555)]).expect("open");
    let bytes = encoded::<Blake3>(&tree);
    let decoded = PartialTree::decode(&Blake3, &bytes).expect("decode");
    assert_eq!(decoded, tree);
    assert_eq!(encoded::<Blake3>(&decoded), bytes);
}

#[test]
fn malformed_and_non_canonical_encodings_are_refused() {
    let set = leaves(8);
    let bytes = encoded::<Blake3>(&sparse::open(&Blake3, &set, &[set[0].0]).expect("open"));
    let decode = |input: &[u8]| PartialTree::decode(&Blake3, input).map(|_| ());

    let mut trailing = bytes.clone();
    trailing.push(0);
    assert_eq!(decode(&trailing), Err(Defect::Malformed("trailing bytes")));
    assert_eq!(
        decode(&bytes[..bytes.len() - 1]),
        Err(Defect::Malformed("truncated"))
    );
    assert_eq!(decode(&[9]), Err(Defect::Malformed("unknown node tag")));

    let mut opaque_empty = vec![TAG_OPAQUE];
    opaque_empty.extend_from_slice(&[0; 32]);
    assert_eq!(
        decode(&opaque_empty),
        Err(Defect::NonCanonical("opaque empty subtree"))
    );

    // A split holding one leaf beside nothing: the leaf, spelled a second way.
    let mut uncollapsed = vec![TAG_INTERNAL, TAG_LEAF];
    uncollapsed.extend_from_slice(&[0; 48]);
    uncollapsed.push(0);
    assert_eq!(
        decode(&uncollapsed),
        Err(Defect::NonCanonical("uncollapsed subtree"))
    );

    // A leaf whose first bit is 1, filed on the 0 side of the root split.
    let mut misplaced = vec![TAG_INTERNAL, TAG_LEAF, 0x80];
    misplaced.extend_from_slice(&[0; 47]);
    misplaced.push(TAG_OPAQUE);
    misplaced.extend_from_slice(&[1; 32]);
    assert_eq!(
        decode(&misplaced),
        Err(Defect::NonCanonical("leaf outside its subtree"))
    );

    let oversized = vec![0; MAX_WITNESS_BYTES + 1];
    assert!(matches!(decode(&oversized), Err(Defect::TooLarge { .. })));
}

#[test]
fn a_split_chain_deeper_than_a_key_is_refused() {
    let mut deep = vec![TAG_INTERNAL; 257];
    deep.push(0);
    assert_eq!(
        PartialTree::decode(&Blake3, &deep).map(|_| ()),
        Err(Defect::NonCanonical("split below the last key bit"))
    );
}

/// Bytes to open `count` keys against a tree of `accounts`.
fn witness_bytes<C: Compress>(compress: &C, accounts: u64, count: u64) -> usize {
    let set = leaves(accounts);
    let touched: Vec<Key> = (0..count).map(|index| key(index * 7 + 1)).collect();
    encoded::<C>(&sparse::open(compress, &set, &touched).expect("open")).len()
}

#[test]
fn a_blake3_opening_of_one_key_is_under_a_kilobyte_at_a_quarter_million_accounts() {
    // log2(262_144) = 18 levels: about 18 × 34 bytes plus the 49-byte leaf.
    let one = witness_bytes(&Blake3, 1 << 18, 1);
    assert!(one < 1_024, "one key: {one} bytes");
    // A transfer opens two keys. They share only the top of their paths, so
    // this is the honest per-transaction figure, and it is over a kilobyte.
    let two = witness_bytes(&Blake3, 1 << 18, 2);
    assert!((1_024..1_536).contains(&two), "two keys: {two} bytes");
}

#[test]
fn the_ring_sis_backend_builds_the_same_trees() {
    let ring = RingSis::new();
    let set = leaves(48);
    let fresh = key(4_242);
    let mut tree = verified(&ring, &set, &[set[5].0, fresh]);
    tree.set(fresh, value(1)).expect("set");
    tree.set(set[5].0, value(2)).expect("set");

    let mut expected = set.clone();
    expected[5].1 = value(2);
    let position = expected
        .binary_search_by(|leaf| leaf.0.cmp(&fresh))
        .expect_err("fresh");
    expected.insert(position, (fresh, value(1)));
    assert_eq!(
        tree.digest(&ring),
        sparse::root(&ring, &expected).expect("root")
    );
}

#[test]
fn a_ring_sis_leaf_is_never_the_empty_digest_and_digests_are_canonical() {
    let ring = RingSis::new();
    assert_ne!(ring.leaf(&[0; 32], &[0; 16]), ring.empty());
    assert_ne!(ring.leaf(&[0; 32], &[0; 16]), ring.leaf(&[0; 32], &[1; 16]));

    // 0x3FFF in the first 14 bits is 16383, which is not below q.
    let mut digest = [0u8; 448];
    digest[0] = 0xFF;
    digest[1] = 0x3F;
    assert_eq!(
        RingSis::read_digest(&digest),
        Err(Defect::NonCanonical("coefficient at or above q"))
    );
}

#[test]
fn a_ring_sis_opening_misses_the_kilobyte_target() {
    // The figure the brief's sub-kilobyte witness runs into: 449 bytes per
    // level. At only 64 accounts one key is already over a kilobyte.
    let ring = RingSis::new();
    let one = witness_bytes(&ring, 64, 1);
    assert!(one > 2_000, "one key: {one} bytes");
}

#[test]
fn an_opened_tree_contains_no_node_it_did_not_need() {
    let set = leaves(64);
    let tree = sparse::open(&Blake3, &set, &[]).expect("open");
    assert!(matches!(tree.root(), Node::Opaque(_)));
}
