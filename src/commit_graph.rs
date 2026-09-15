use std::{collections::HashMap, ops::{ControlFlow, Index}};

use crate::{Author, CommitHash, CommitMeta, Signature};

pub struct CommitGraph<P> {
    nodes: NodeStore,
    edges: EdgeStore,
    parents: Vec<Option<EdgeIdx>>,
    store: HashMap<NodeIdx, P>,
}

struct Node {
    meta: CommitMeta,
    parents: Vec<NodeIdx>,
}


impl<P> CommitGraph<P> {
    pub fn new() -> Self {
        Self{
            nodes: NodeStore(Vec::new()),
            edges: EdgeStore(Vec::new()),
            store: HashMap::new(),
            parents: Vec::new(),
        }
    }

    pub fn parents(&self, node_idx: NodeIdx) -> impl Iterator<Item = NodeIdx> {
        let mut edge_idx = self.parents[node_idx.0 as usize];
        std::iter::from_fn(move || {
            let this_edge_idx = edge_idx?;
            let edge = &self.edges[this_edge_idx];
            edge_idx = edge.next;
            Some(edge.target)
        })
    }

    pub fn children(&self, node_idx: NodeIdx) -> impl Iterator<Item = NodeIdx> {
        self.parents.iter().enumerate().filter_map(move |(i, parent_edge_idx)| {
            let parent_edge_idx = *parent_edge_idx;
            if let Some(parent_edge_idx) = parent_edge_idx {
                let edge = &self.edges[parent_edge_idx];
                if edge.target == node_idx {
                    return Some(NodeIdx(i));
                }
            }
            None
        })
    }

    pub fn roots(&self) -> impl Iterator<Item = NodeIdx> {
        self.parents.iter().filter(|e| e.is_none()).enumerate().map(|(i, _)| NodeIdx(i))
    }

    pub fn ingest_commit(&mut self, meta: CommitMeta, data: P) {
        let node_idx = NodeIdx(self.nodes.0.len());
        self.store.insert(node_idx, data);
        self.nodes.0.push(Node {
            meta,
            parents: Vec::new(),
        });
    }

    pub fn traverse<F: FnMut(&CommitMeta, &P) -> ControlFlow<()>>(&self, mut traversal: F) {
        let mut stack = self.roots().collect::<Vec::<_>>();
        while let Some(next) = stack.pop() {
            let node = &self.nodes[next];
            if let ControlFlow::Continue(_) = traversal(&node.meta, &self.store[&next]) {
                stack.extend(self.children(next));
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct NodeIdx(usize);

struct NodeStore(Vec<Node>);

impl<'a> IntoIterator for &'a NodeStore {
    type Item = (NodeIdx, &'a Node);

    type IntoIter = NodeIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        NodeIter(self.0.iter().enumerate())
    }
}

struct NodeIter<'a>(std::iter::Enumerate<std::slice::Iter<'a, Node>>);

impl<'a> Iterator for NodeIter<'a> {
    type Item = (NodeIdx, &'a Node);

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|(count, node)| (NodeIdx(count), node))
    }
}

impl Index<NodeIdx> for NodeStore {
    type Output = Node;

    fn index(&self, index: NodeIdx) -> &Self::Output {
        &self.0[index.0]
    }
}

struct Edge {
    next: Option<EdgeIdx>,
    target: NodeIdx,
}

struct EdgeStore(Vec<Edge>);
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct EdgeIdx(usize);

impl Index<EdgeIdx> for EdgeStore {
    type Output = Edge;

    fn index(&self, index: EdgeIdx) -> &Self::Output {
        &self.0[index.0]
    }


}
