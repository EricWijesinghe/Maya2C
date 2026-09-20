//! A partial tree: the witness, decoded, and the thing a stateless node edits.
//!
//! It holds the nodes on the opened paths and an opaque digest for every
//! subtree off them. Reading or writing a key under an opaque digest is
//! [`Defect::Unopened`], never a guess. Writes rebuild nodes in place, so a
//! second transfer in a block reads the first one's result — the intra-block
//! staleness a per-transaction witness cannot handle.
//!
//! A tree is only worth reading once its digest has matched a committed root.
//! [`PartialTree::verify`] is the only way to a [`VerifiedTree`], and only a
//! verified tree executes transfers: a value read from an unchecked witness is
//! a value a relay chose, and a violation computed from it would let the relay
//! declare an honest block invalid.

use crate::compress::{Compress, Key, Value};
use crate::error::Defect;
use crate::params::{
    KEY_BITS, KEY_BYTES, MAX_WITNESS_BYTES, MAX_WITNESS_NODES, TAG_EMPTY, TAG_INTERNAL, TAG_LEAF,
    TAG_OPAQUE, VALUE_BYTES,
};
use crate::sparse::bit;

/// One node of a partial tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node<D> {
    /// A subtree holding no leaves.
    Empty,
    /// A subtree the witness does not open.
    Opaque(D),
    /// A subtree holding exactly this leaf.
    Leaf(Key, Value),
    /// A subtree split on the next key bit: zero left, one right.
    Internal(Box<Node<D>>, Box<Node<D>>),
}

/// A witness's tree, not yet checked against any root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartialTree<D> {
    root: Node<D>,
}

/// A partial tree whose digest matched a committed root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedTree<D> {
    tree: PartialTree<D>,
}

impl<D: Copy + Eq> PartialTree<D> {
    pub(crate) const fn from_root(root: Node<D>) -> Self {
        Self { root }
    }

    /// The root node.
    #[must_use]
    pub const fn root(&self) -> &Node<D> {
        &self.root
    }

    /// The digest this tree reproduces.
    pub fn digest<C: Compress<Digest = D>>(&self, compress: &C) -> D {
        node_digest(compress, &self.root)
    }

    /// Checks the digest with `accepts`, which sees the accounts root.
    ///
    /// A predicate rather than an expected digest, because the node's state
    /// root folds further layers above the accounts root and this crate does
    /// not know them.
    ///
    /// # Errors
    ///
    /// [`Defect::RootMismatch`] if `accepts` refuses the digest.
    pub fn verify<C: Compress<Digest = D>>(
        self,
        compress: &C,
        accepts: impl FnOnce(&D) -> bool,
    ) -> Result<VerifiedTree<D>, Defect> {
        if accepts(&self.digest(compress)) {
            Ok(VerifiedTree { tree: self })
        } else {
            Err(Defect::RootMismatch)
        }
    }

    /// Canonical encoding: the nodes in preorder, each a tag and its payload.
    pub fn encode<C: Compress<Digest = D>>(&self, out: &mut Vec<u8>) {
        encode_node::<C>(&self.root, out);
    }

    /// Decodes the one canonical encoding of a partial tree.
    ///
    /// # Errors
    ///
    /// [`Defect::TooLarge`] past [`MAX_WITNESS_BYTES`] or
    /// [`MAX_WITNESS_NODES`]; [`Defect::Malformed`] on truncation, trailing
    /// bytes or an unknown tag; [`Defect::NonCanonical`] for a leaf outside its
    /// subtree, a subtree of at most one leaf left uncollapsed, an opaque empty
    /// subtree, or a split below the last key bit.
    pub fn decode<C: Compress<Digest = D>>(compress: &C, bytes: &[u8]) -> Result<Self, Defect> {
        if bytes.len() > MAX_WITNESS_BYTES {
            return Err(Defect::TooLarge {
                limit: MAX_WITNESS_BYTES,
                found: bytes.len(),
            });
        }
        let mut decoder = Decoder {
            bytes,
            position: 0,
            nodes: 0,
            path: [0; KEY_BYTES],
        };
        let root = decoder.node(compress, 0)?;
        if decoder.position != bytes.len() {
            return Err(Defect::Malformed("trailing bytes"));
        }
        Ok(Self { root })
    }
}

impl<D: Copy + Eq> VerifiedTree<D> {
    /// The value under `key`, `None` if the tree proves it absent.
    ///
    /// # Errors
    ///
    /// [`Defect::Unopened`] if `key` lies under an opaque subtree.
    pub fn get(&self, key: &Key) -> Result<Option<Value>, Defect> {
        let mut node = &self.tree.root;
        let mut depth = 0;
        loop {
            match node {
                Node::Empty => return Ok(None),
                Node::Opaque(_) => return Err(Defect::Unopened { key: *key }),
                Node::Leaf(stored, value) => return Ok((stored == key).then_some(*value)),
                Node::Internal(left, right) => {
                    if depth >= KEY_BITS {
                        return Err(Defect::NonCanonical("split below the last key bit"));
                    }
                    node = if bit(key, depth) { right } else { left };
                    depth += 1;
                }
            }
        }
    }

    /// Writes `value` under `key`, inserting the leaf if it is absent.
    ///
    /// # Errors
    ///
    /// [`Defect::Unopened`] if `key` lies under an opaque subtree.
    pub fn set(&mut self, key: Key, value: Value) -> Result<(), Defect> {
        set_at(&mut self.tree.root, key, value, 0)
    }

    /// The digest after every write so far.
    pub fn digest<C: Compress<Digest = D>>(&self, compress: &C) -> D {
        self.tree.digest(compress)
    }
}

fn node_digest<C: Compress>(compress: &C, node: &Node<C::Digest>) -> C::Digest {
    match node {
        Node::Empty => compress.empty(),
        Node::Opaque(digest) => *digest,
        Node::Leaf(key, value) => compress.leaf(key, value),
        Node::Internal(left, right) => {
            compress.internal(&node_digest(compress, left), &node_digest(compress, right))
        }
    }
}

fn set_at<D>(node: &mut Node<D>, key: Key, value: Value, depth: usize) -> Result<(), Defect> {
    match node {
        Node::Opaque(_) => Err(Defect::Unopened { key }),
        Node::Empty => {
            *node = Node::Leaf(key, value);
            Ok(())
        }
        Node::Leaf(stored, current) if *stored == key => {
            *current = value;
            Ok(())
        }
        Node::Leaf(stored, current) => {
            let (other, other_value) = (*stored, *current);
            *node = split(other, other_value, key, value, depth);
            Ok(())
        }
        Node::Internal(left, right) => {
            if depth >= KEY_BITS {
                return Err(Defect::NonCanonical("split below the last key bit"));
            }
            let child = if bit(&key, depth) { right } else { left };
            set_at(child, key, value, depth + 1)
        }
    }
}

/// The subtree at `depth` holding two distinct leaves: a chain of one-sided
/// splits down to the first bit where the keys differ.
fn split<D>(other: Key, other_value: Value, key: Key, value: Value, depth: usize) -> Node<D> {
    let mut divergence = depth;
    while divergence < KEY_BITS && bit(&other, divergence) == bit(&key, divergence) {
        divergence += 1;
    }
    let (low, high) = if bit(&key, divergence) {
        (Node::Leaf(other, other_value), Node::Leaf(key, value))
    } else {
        (Node::Leaf(key, value), Node::Leaf(other, other_value))
    };
    let mut node = Node::Internal(Box::new(low), Box::new(high));
    for level in (depth..divergence).rev() {
        node = if bit(&key, level) {
            Node::Internal(Box::new(Node::Empty), Box::new(node))
        } else {
            Node::Internal(Box::new(node), Box::new(Node::Empty))
        };
    }
    node
}

fn encode_node<C: Compress>(node: &Node<C::Digest>, out: &mut Vec<u8>) {
    match node {
        Node::Empty => out.push(TAG_EMPTY),
        Node::Opaque(digest) => {
            out.push(TAG_OPAQUE);
            C::write_digest(digest, out);
        }
        Node::Leaf(key, value) => {
            out.push(TAG_LEAF);
            out.extend_from_slice(key);
            out.extend_from_slice(value);
        }
        Node::Internal(left, right) => {
            out.push(TAG_INTERNAL);
            encode_node::<C>(left, out);
            encode_node::<C>(right, out);
        }
    }
}

struct Decoder<'a> {
    bytes: &'a [u8],
    position: usize,
    nodes: usize,
    /// The key bits spelled by the path to the node being decoded.
    path: Key,
}

impl<'a> Decoder<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], Defect> {
        let end = self.position + len;
        let slice = self
            .bytes
            .get(self.position..end)
            .ok_or(Defect::Malformed("truncated"))?;
        self.position = end;
        Ok(slice)
    }

    fn node<C: Compress>(&mut self, compress: &C, depth: usize) -> Result<Node<C::Digest>, Defect> {
        self.nodes += 1;
        if self.nodes > MAX_WITNESS_NODES {
            return Err(Defect::TooLarge {
                limit: MAX_WITNESS_NODES,
                found: self.nodes,
            });
        }
        match self.take(1)?[0] {
            TAG_EMPTY => Ok(Node::Empty),
            TAG_OPAQUE => {
                let digest = C::read_digest(self.take(C::DIGEST_BYTES)?)?;
                if digest == compress.empty() {
                    return Err(Defect::NonCanonical("opaque empty subtree"));
                }
                Ok(Node::Opaque(digest))
            }
            TAG_LEAF => {
                let key: Key = self
                    .take(KEY_BYTES)?
                    .try_into()
                    .map_err(|_| Defect::Malformed("key"))?;
                let value: Value = self
                    .take(VALUE_BYTES)?
                    .try_into()
                    .map_err(|_| Defect::Malformed("value"))?;
                if !(0..depth).all(|level| bit(&key, level) == bit(&self.path, level)) {
                    return Err(Defect::NonCanonical("leaf outside its subtree"));
                }
                Ok(Node::Leaf(key, value))
            }
            TAG_INTERNAL => self.internal(compress, depth),
            _ => Err(Defect::Malformed("unknown node tag")),
        }
    }

    fn internal<C: Compress>(
        &mut self,
        compress: &C,
        depth: usize,
    ) -> Result<Node<C::Digest>, Defect> {
        if depth >= KEY_BITS {
            return Err(Defect::NonCanonical("split below the last key bit"));
        }
        let mask = 1u8 << (7 - depth % 8);
        self.path[depth / 8] &= !mask;
        let left = self.node(compress, depth + 1)?;
        self.path[depth / 8] |= mask;
        let right = self.node(compress, depth + 1)?;
        self.path[depth / 8] &= !mask;

        // A subtree of at most one leaf is that leaf or nothing. Spelled as a
        // split it would be a second encoding of the same tree — and with a
        // different digest, a tree nobody committed to.
        if matches!(
            (&left, &right),
            (Node::Empty | Node::Leaf(..), Node::Empty) | (Node::Empty, Node::Leaf(..))
        ) {
            return Err(Defect::NonCanonical("uncollapsed subtree"));
        }
        Ok(Node::Internal(Box::new(left), Box::new(right)))
    }
}
