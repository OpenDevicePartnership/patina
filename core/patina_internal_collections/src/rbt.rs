//! Slice Collections - Red-Black Tree
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
#[cfg(feature = "alloc")]
extern crate alloc;

use crate::{
    SliceKey,
    node::{Node, NodeTrait, Storage},
};

use super::{Error, Result};
use core::{
    cmp::Ordering,
    ptr,
    sync::atomic::{self, AtomicPtr},
};

/// A red-black tree that can hold up to `SIZE` nodes.
///
/// The tree is implemented using the [AtomicPtr] structure, so the target must support atomic operations.
pub struct Rbt<'a, D>
where
    D: SliceKey,
{
    storage: Storage<'a, D>,
    root: AtomicPtr<Node<D>>,
}

impl<'a, D> Rbt<'a, D>
where
    D: SliceKey + 'a,
{
    /// Creates a zero capacity red-black tree.
    ///
    /// This is useful for creating a tree at compile time and replacing the memory later. Use
    /// [with_capacity](Self::with_capacity) to create a tree with a given slice of memory immediately. Otherwise use
    /// [resize](Self::resize) to replace the memory later.
    pub const fn new() -> Self {
        Rbt { storage: Storage::new(), root: AtomicPtr::new(core::ptr::null_mut()) }
    }

    /// Creates a new binary tree with a given slice of memory.
    pub fn with_capacity(slice: &'a mut [u8]) -> Self {
        Rbt { storage: Storage::with_capacity(slice), root: AtomicPtr::default() }
    }

    /// Returns the number of elements in the tree.
    pub fn len(&self) -> usize {
        self.storage.len()
    }

    /// Indicates whether the tree is empty.
    pub fn is_empty(&self) -> bool {
        self.storage.len() == 0
    }

    /// Returns the capacity of the tree.
    pub fn capacity(&self) -> usize {
        self.storage.capacity()
    }

    /// Returns the height of the tree.
    pub fn height(&self) -> i32 {
        let (height, _) = Node::height_and_balance(self.root());
        height
    }

    /// Returns the root of the tree.
    fn root(&self) -> Option<&Node<D>> {
        let root_ptr = self.root.load(atomic::Ordering::SeqCst);
        if root_ptr.is_null() {
            return None;
        }
        Some(unsafe { &*root_ptr })
    }

    /// Adds a value into the tree.
    ///
    /// # Time Complexity
    ///
    /// O(log n) for a balanced tree.
    ///
    /// # Errors
    ///
    /// Returns [AlreadyExists](Error::AlreadyExists) if the value already exists in the tree.
    ///
    /// Returns [OutOfSpace](Error::OutOfSpace) if the storage is full.
    ///
    pub fn add(&mut self, data: D) -> Result<usize> {
        let (idx, node) = self.storage.add(data)?;
        node.set_red();

        if self.root.load(atomic::Ordering::SeqCst).is_null() {
            node.set_black();
            self.root.store(node, atomic::Ordering::SeqCst);
            return Ok(idx);
        }

        let root = unsafe { &mut *self.root.load(atomic::Ordering::SeqCst) };

        Self::add_node(root, node)?;
        Self::fixup_add(&self.root, node);

        Ok(idx)
    }

    /// Adds many values into the tree.
    ///
    /// # Time Complexity
    ///
    /// O(m log n) for a balanced tree, where m is the number of values to add.
    ///
    pub fn add_many<I>(&mut self, data: I) -> Result<usize>
    where
        I: IntoIterator<Item = D>,
        I::IntoIter: ExactSizeIterator,
    {
        let data = data.into_iter();

        if self.len() + data.len() > self.capacity() {
            return Err(Error::OutOfSpace);
        }
        let mut idx = 0;
        for d in data {
            idx = self.add(d)?;
        }
        Ok(idx)
    }

    /// adds a node into the tree. The node must already exist in the storage.
    fn add_node(start: &Node<D>, node: &Node<D>) -> Result<()> {
        let mut current = start;
        loop {
            match node.key().cmp(current.key()) {
                Ordering::Less => match current.left() {
                    Some(left) => current = left,
                    None => {
                        current.set_left(Some(node));
                        node.set_parent(Some(current));
                        return Ok(());
                    }
                },
                Ordering::Greater => match current.right() {
                    Some(right) => current = right,
                    None => {
                        current.set_right(Some(node));
                        node.set_parent(Some(current));
                        return Ok(());
                    }
                },
                Ordering::Equal => return Err(Error::AlreadyExists),
            }
        }
    }

    /// Searches for a value in the tree, returning it if it exists.
    ///
    /// Returns `Some(D)` if the value was found.
    ///
    /// Returns `None` if the value was not found.
    ///
    /// # Time Complexity
    ///
    /// O(log n) for a balanced tree. Use [get_with_idx](Self::get_with_idx)
    /// if you know the index, as it is O(1).
    ///
    pub fn get(&self, key: &D::Key) -> Option<&D> {
        match self.get_node(key) {
            Some(node) => Some(&node.data),
            None => None,
        }
    }

    /// Searches for a value in the tree, returning a mutable reference to it if it exists.
    ///
    /// Returns `Some(&mut D)` if the value was found.
    ///
    /// Returns `None` if the value was not found.
    ///
    /// # Time Complexity
    ///
    /// O(log n) for a balanced tree.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the mutable reference is not used to modify any value that
    /// affects the value of the key.
    ///
    pub unsafe fn get_mut(&mut self, key: &D::Key) -> Option<&mut D> {
        match self.get_node(key) {
            Some(node) => Some(unsafe { &mut (*node.as_mut_ptr()).data }),
            None => None,
        }
    }

    /// Directly accesses a value from the underlying storage.
    ///
    /// The node returned is not guaranteed to be in the tree nor is it guaranteed to be the same
    /// node as was added when `index` was returned from [add](Self::add). This is because
    /// deleting nodes from the tree does not free the memory in storage, only marks it to be
    /// reused.
    ///
    /// Returns `Some(D)` if the value was found.
    ///
    /// Returns `None` if the value was not found.
    ///
    /// # Time Complexity
    ///
    /// O(1)
    ///
    pub fn get_with_idx(&self, idx: usize) -> Option<&D> {
        match self.storage.get(idx) {
            Some(node) => Some(&node.data),
            None => None,
        }
    }

    /// Directly accesses a value from the underlying storage.
    ///
    /// The node returned is not guaranteed to be in the tree nor is it guaranteed to be the same
    /// node as was added when `index` was returned from [add](Self::add). This is because
    /// deleting nodes from the tree does not free the memory in storage, only marks it to be
    /// reused.
    ///
    /// Returns `Some(D)` if the value was found.
    ///
    /// Returns `None` if the value was not found.
    ///
    /// # Time Complexity
    ///
    /// O(1)
    ///
    /// # Safety
    ///
    /// The caller must ensure that the mutable reference is not used to modify any value that
    /// affects the value of the key.
    ///
    pub unsafe fn get_with_idx_mut(&mut self, idx: usize) -> Option<&mut D> {
        match self.storage.get_mut(idx) {
            Some(node) => Some(&mut node.data),
            None => None,
        }
    }

    /// Searches the tree, returning the index of the value if it exists.
    ///
    /// The index returned should only be used for immediate direct access to the value in storage
    /// and should not be stored for later use the underlying node is not guaranteed to be in the
    /// tree nor is it guaranteed to be the same node as when `index` was retrieved. This is
    /// because deleting nodes from the tree does not free the memory in storage, only marks it to be
    /// reused.
    ///
    /// Returns `Some(usize)` if the value was found.
    ///
    /// Returns `None` if the value was not found.
    ///
    /// # Time Complexity
    ///
    /// O(log n) for a balanced tree.
    ///
    pub fn get_idx(&self, key: &D::Key) -> Option<usize> {
        self.get_node(key).map(|node| self.storage.idx(node.as_mut_ptr()))
    }

    /// Searches the tree, returning the closest value to the given key, rounded down.
    ///
    /// The index returned should only be used for immediate direct access to the value in storage
    /// and should not be stored for later use the underlying node is not guaranteed to be in the
    /// tree nor is it guaranteed to be the same node as when `index` was retrieved. This is
    /// because deleting nodes from the tree does not free the memory in storage, only marks it to be
    /// reused.
    ///
    /// Returns `Some(usize)` if the value was found.
    ///
    /// Returns `None` if the value was not found.
    ///
    /// # Time Complexity
    ///
    /// O(log n) for a balanced tree.
    ///
    pub fn get_closest_idx(&self, key: &D::Key) -> Option<usize> {
        let mut current = self.root();
        let mut closest = None;
        while let Some(node) = current {
            match key.cmp(node.data.key()) {
                Ordering::Equal => return Some(self.storage.idx(node.as_mut_ptr())),
                Ordering::Less => current = node.left(),
                Ordering::Greater => {
                    closest = Some(node);
                    current = node.right();
                }
            }
        }
        closest.map(|node| self.storage.idx(node.as_mut_ptr()))
    }

    /// Returns the first ordered value in the tree.
    ///
    /// Returns `Some(D)` if the value was found.
    ///
    /// Returns `None` if the tree is empty.
    ///
    /// # Time Complexity
    ///
    /// O(log n) for a balanced tree.
    ///
    pub fn first(&self) -> Option<&D> {
        let idx = self.first_idx()?;
        self.get_with_idx(idx)
    }

    /// Returns the last ordered value in the tree.
    ///
    /// Returns `Some(D)` if the value was found.
    ///
    /// Returns `None` if the tree is empty.
    ///
    /// # Time Complexity
    ///
    /// O(log n) for a balanced tree.
    ///
    pub fn last(&self) -> Option<&D> {
        let idx = self.last_idx()?;
        self.get_with_idx(idx)
    }

    /// Returns the index of the first ordered value in the tree.
    ///
    /// The index returned should only be used for immediate direct access to the value in storage
    /// and should not be stored for later use the underlying node is not guaranteed to be in the
    /// tree nor is it guaranteed to be the same node as when `index` was retrieved. This is
    /// because deleting nodes from the tree does not free the memory in storage, only marks it to be
    /// reused.
    ///
    /// Returns `Some(usize)` if the value was found.
    ///
    /// Returns `None` if the tree is empty.
    ///
    /// # Time Complexity
    ///
    /// O(log n) for a balanced tree.
    ///
    pub fn first_idx(&self) -> Option<usize> {
        let mut current = self.root();
        while let Some(node) = current {
            if node.left().is_none() {
                return Some(self.storage.idx(node.as_mut_ptr()));
            }
            current = node.left();
        }
        None
    }

    /// Returns the index of the last ordered value in the tree.
    ///
    /// The index returned should only be used for immediate direct access to the value in storage
    /// and should not be stored for later use the underlying node is not guaranteed to be in the
    /// tree nor is it guaranteed to be the same node as when `index` was retrieved. This is
    /// because deleting nodes from the tree does not free the memory in storage, only marks it to be
    /// reused.
    ///
    /// Returns `Some(usize)` if the value was found.
    ///
    /// Returns `None` if the tree is empty.
    ///
    /// # Time Complexity
    ///
    /// O(log n) for a balanced tree.
    ///
    pub fn last_idx(&self) -> Option<usize> {
        let mut current = self.root();
        while let Some(node) = current {
            if node.right().is_none() {
                return Some(self.storage.idx(node.as_mut_ptr()));
            }
            current = node.right();
        }
        None
    }

    /// Returns the next ordered value in the tree.
    ///
    /// Returns `Some(D)` if the value was found.
    ///
    /// Returns `None` if the value was not found.
    ///
    /// # Time Complexity
    ///
    /// O(log n) for a balanced tree.
    ///
    pub fn next(&self, current: D) -> Option<&D> {
        let idx = self.get_idx(current.key())?;
        let next_idx = self.next_idx(idx)?;
        self.get_with_idx(next_idx)
    }

    /// Returns the previous ordered value in the tree.
    ///
    /// Returns `Some(D)` if the value was found.
    ///
    /// Returns `None` if the value was not found.
    ///
    /// # Time Complexity
    ///
    /// O(log n) for a balanced tree.
    ///
    pub fn prev(&self, current: D) -> Option<&D> {
        let idx = self.get_idx(current.key())?;
        let prev_idx = self.prev_idx(idx)?;
        self.get_with_idx(prev_idx)
    }

    /// Returns the index of the next ordered value in the tree.
    ///
    /// The index returned should only be used for immediate direct access to the value in storage
    /// and should not be stored for later use the underlying node is not guaranteed to be in the
    /// tree nor is it guaranteed to be the same node as when `index` was retrieved. This is
    /// because deleting nodes from the tree does not free the memory in storage, only marks it to be
    /// reused.
    ///
    /// Returns `Some(usize)` if the value was found.
    ///
    /// Returns `None` if the value was not found.
    ///
    /// # Time Complexity
    ///
    /// O(1) ~ O(log n) for a balanced tree.
    ///
    pub fn next_idx(&self, current: usize) -> Option<usize> {
        let node = self.storage.get(current)?;

        if node.right().is_some() {
            let successor = Node::successor(node)?;
            let idx = self.storage.idx(successor.as_mut_ptr());
            return Some(idx);
        }

        let mut current = node;
        while let Some(parent) = current.parent() {
            if parent.left_ptr() == current.as_mut_ptr() {
                let idx = self.storage.idx(parent.as_mut_ptr());
                return Some(idx);
            }
            current = parent;
        }
        None
    }

    /// Returns the index of the previous ordered value in the tree.
    ///
    /// The index returned should only be used for immediate direct access to the value in storage
    /// and should not be stored for later use the underlying node is not guaranteed to be in the
    /// tree nor is it guaranteed to be the same node as when `index` was retrieved. This is
    /// because deleting nodes from the tree does not free the memory in storage, only marks it to be
    /// reused.
    ///
    /// Returns `Some(usize)` if the value was found.
    ///
    /// Returns `None` if the value was not found.
    ///
    /// # Time Complexity
    ///
    /// O(1) ~ O(log n) for a balanced tree.
    ///
    pub fn prev_idx(&self, current: usize) -> Option<usize> {
        let node = self.storage.get(current)?;

        if node.left().is_some() {
            let predecessor = Node::predecessor(node)?;
            let idx = self.storage.idx(predecessor.as_mut_ptr());
            return Some(idx);
        }

        let mut current = node;
        while let Some(parent) = current.parent() {
            if parent.right_ptr() == current.as_mut_ptr() {
                let idx = self.storage.idx(parent.as_mut_ptr());
                return Some(idx);
            }
            current = parent;
        }
        None
    }

    /// Gets a value from the tree given the key.
    ///
    /// Returns `Some(Node<D>)` if the value was found.
    ///
    /// Returns `None` if the value was not found.
    ///
    /// # Time Complexity
    ///
    /// O(log n) for a balanced tree.
    ///
    fn get_node(&self, key: &D::Key) -> Option<&Node<D>> {
        let mut current_idx = self.root();
        while let Some(node) = current_idx {
            match key.cmp(node.key()) {
                Ordering::Equal => return Some(node),
                Ordering::Less => current_idx = node.left(),
                Ordering::Greater => current_idx = node.right(),
            }
        }
        None
    }

    /// Deletes a value from the tree from the given key.
    ///
    /// Returns `Some(D)` if the value was found and deleted.
    ///
    /// Returns `None` if the value was not found.
    ///
    /// # Time Complexity
    ///
    /// O(log n) for a balanced tree.
    ///
    pub fn delete(&mut self, key: &D::Key) -> Result<()> {
        let to_delete = match self.get_node(key) {
            Some(node) => node,
            None => return Err(Error::NotFound),
        };

        Self::remove_node_from_tree(&self.root, to_delete);

        self.storage.delete(to_delete.as_mut_ptr());
        Ok(())
    }

    /// Deletes a value from the tree located at the given index.
    ///
    /// Returns `Some(D)` if the value was found and deleted.
    ///
    /// Returns `None` if the value was not found.
    ///
    /// # Time Complexity
    ///
    /// O(1)
    ///
    pub fn delete_with_idx(&mut self, idx: usize) -> Result<()> {
        let to_delete = match self.storage.get(idx) {
            Some(node) => node,
            None => return Err(Error::NotFound),
        };
        Self::remove_node_from_tree(&self.root, to_delete);

        self.storage.delete(to_delete.as_mut_ptr());
        Ok(())
    }

    /// Removes a node in the tree.
    fn remove_node_from_tree<'b>(root: &'b AtomicPtr<Node<D>>, to_delete: &'b Node<D>) {
        //} -> &'b Node<D> {
        // if both children are null, fixup the tree first so rotates work as expected,
        // then remove the node.
        if to_delete.left().is_none() && to_delete.right().is_none() {
            Self::fixup_delete(root, Some(to_delete));
            Self::remove_node_with_zero_or_one_child(to_delete);
            if to_delete.parent().is_none() {
                root.store(ptr::null_mut(), atomic::Ordering::SeqCst);
            }
            return;
        }

        let moved_up;
        // If one child exists, simply remove the node.
        if to_delete.left().is_none() || to_delete.right().is_none() {
            moved_up = Self::remove_node_with_zero_or_one_child(to_delete);
            if to_delete.parent().is_none() {
                root.store(moved_up.as_mut_ptr(), atomic::Ordering::SeqCst);
                moved_up.set_parent(None);
            }
        }
        // if two children exist, find the successor and replace the value of the node, then removing the successor.
        else {
            let successor = Node::successor(to_delete).expect("to_delete has both children");

            Node::swap(to_delete, successor);
            if successor.parent().is_none() {
                root.store(successor.as_mut_ptr(), atomic::Ordering::SeqCst);
                successor.set_parent(None);
            }

            // to_delete must have a parent due to the successor swap, no need
            // to check if we need to update the head.
            moved_up = Self::remove_node_with_zero_or_one_child(to_delete);
        }

        if to_delete.is_black() {
            Self::fixup_delete(root, moved_up);
        }
    }

    /// Removes a node with zero or one child from the tree.
    fn remove_node_with_zero_or_one_child(node: &Node<D>) -> Option<&Node<D>> {
        let parent = node.parent();

        if node.left().is_some() {
            node.left().set_parent(parent);
            if parent.left_ptr() == node.as_mut_ptr() {
                parent.set_left(node.left());
            } else {
                parent.set_right(node.left());
            }
            return node.left();
        }

        if node.right().is_some() {
            node.right().set_parent(parent);
            if parent.left_ptr() == node.as_mut_ptr() {
                parent.set_left(node.right());
            } else {
                parent.set_right(node.right());
            }
            return node.right();
        }

        if parent.left_ptr() == node.as_mut_ptr() {
            parent.set_left(None);
        } else if parent.right_ptr() == node.as_mut_ptr() {
            parent.set_right(None);
        }
        None
    }

    /// Rotate the subtree to the left and return the new root.
    fn rotate_left(node: &Node<D>) -> Option<&Node<D>> {
        let right_child = node.right();
        let parent_tmp = node.parent();

        node.set_right(right_child.left());
        right_child.left().set_parent(Some(node));

        right_child.set_left(Some(node));
        node.set_parent(right_child);

        right_child.set_parent(parent_tmp);
        if parent_tmp.left_ptr() == node.as_mut_ptr() {
            parent_tmp.set_left(right_child);
        } else if parent_tmp.right_ptr() == node.as_mut_ptr() {
            parent_tmp.set_right(right_child);
        }
        right_child
    }

    /// Rotate the subtree to the right and return the new root.
    fn rotate_right(node: &Node<D>) -> Option<&Node<D>> {
        let left_child = node.left();
        let parent_tmp = node.parent();

        node.set_left(left_child.right());
        left_child.right().set_parent(Some(node));

        left_child.set_right(Some(node));
        node.set_parent(left_child);

        left_child.set_parent(parent_tmp);
        if parent_tmp.left_ptr() == node.as_mut_ptr() {
            parent_tmp.set_left(left_child);
        } else if parent_tmp.right_ptr() == node.as_mut_ptr() {
            parent_tmp.set_right(left_child);
        }
        left_child
    }

    /// Updates the tree after a node has been added, to meet the red-black tree properties.
    fn fixup_add(head: &AtomicPtr<Node<D>>, node: &Node<D>) {
        // Case 1: The node is the root of the tree, no fixups needed.
        let Some(mut parent) = node.parent() else {
            node.set_black();
            return;
        };

        // The parent is black, no fixups needed.
        if parent.is_black() {
            return;
        }

        // Case 2: is enforced by setting the parent to black. If the parent is red, the grandparent should exist.
        let grandparent = parent.parent().expect("Parent is red, grandparent should exist");
        let uncle = Node::sibling(parent);

        // Case 3: Uncle is red, recolor parent, grandparent, uncle
        if uncle.is_red() {
            parent.set_black();
            grandparent.set_red();
            uncle.set_black();

            // Recursively fixup the grandparent
            Self::fixup_add(head, grandparent);
        }
        // Parent is left child of grandparent
        else if parent.as_mut_ptr() == grandparent.left_ptr() {
            // Case 4a: uncle is black and node is left->right "inner child" of it's grandparent
            if node.as_mut_ptr() == parent.right_ptr() {
                if let Some(root) = Self::rotate_left(parent)
                    && root.parent().is_none()
                {
                    head.store(root.as_mut_ptr(), atomic::Ordering::SeqCst);
                    root.set_parent(None);
                }
                parent = node;
            }
            // Case 5a: uncle is black and node is left->left "outer child" of it's grandparent
            if let Some(root) = Self::rotate_right(grandparent)
                && root.parent().is_none()
            {
                head.store(root.as_mut_ptr(), atomic::Ordering::SeqCst);
                root.set_parent(None);
            }
            parent.set_black();
            grandparent.set_red();
        }
        // Parent is right child of grandparent
        else if parent.as_mut_ptr() == grandparent.right_ptr() {
            // Case 4b: uncle is black and node is right->left "inner child" of its grandparent
            if node.as_mut_ptr() == parent.left_ptr() {
                if let Some(root) = Self::rotate_right(parent)
                    && root.parent().is_none()
                {
                    head.store(root.as_mut_ptr(), atomic::Ordering::SeqCst);
                    root.set_parent(None);
                }
                parent = node;
            }
            if let Some(root) = Self::rotate_left(grandparent)
                && root.parent().is_none()
            {
                head.store(root.as_mut_ptr(), atomic::Ordering::SeqCst);
                root.set_parent(None);
            }

            parent.set_black();
            grandparent.set_red();
        } else {
            // Broken Tree, unrecoverable
            panic!("Parent is not a child of grandparent")
        }
    }

    /// Updates the tree after a node has been deleted, to meet the red-black tree properties.
    fn fixup_delete(root: &AtomicPtr<Node<D>>, node: Option<&Node<D>>) {
        // Case 1: The node is the root of the tree, no fixups needed.
        if node.parent().is_none() {
            node.set_black();
            return;
        }

        let node = node.expect("Node exists");

        let mut sibling = Node::sibling(node);

        // Case 2: The sibling is red
        if sibling.is_red() {
            sibling.set_black();
            node.parent().set_red();
            if node.parent().left_ptr() == node.as_mut_ptr() {
                if let Some(subtree_root) = Self::rotate_left(node.parent().expect("Parent exists"))
                    && subtree_root.parent().is_none()
                {
                    root.store(subtree_root.as_mut_ptr(), atomic::Ordering::SeqCst);
                    subtree_root.set_parent(None);
                }
            } else if let Some(subtree_root) = Self::rotate_right(node.parent().expect("Parent exists"))
                && subtree_root.parent().is_none()
            {
                root.store(subtree_root.as_mut_ptr(), atomic::Ordering::SeqCst);
                subtree_root.set_parent(None);
            }

            sibling = Node::sibling(node); // Update sibling for fall through cases 3-6
        }

        // Cases 3+4: Black sibling with two black children
        if sibling.left().is_black() && sibling.right().is_black() {
            sibling.set_red();

            // Case 3: Black sibling with two black children + red parent
            if node.parent().is_red() {
                node.parent().set_black();
            }
            // Case 4: Black sibling with two black children + black parent
            else {
                Self::fixup_delete(root, node.parent());
            }
        }
        // Case 5+6: Black sibling with at least one red child
        else {
            let node_is_left_child = node.as_mut_ptr() == node.parent().left_ptr();

            // Case 5: Black sibling with at least one red child + "outer nephew" is black
            // Recolor sibling and its child, rotate around sibling
            if node_is_left_child && sibling.right().is_black() {
                sibling.left().set_black();
                sibling.set_red();
                if let Some(subtree_root) = Self::rotate_right(sibling.unwrap())
                    && subtree_root.parent().is_none()
                {
                    root.store(subtree_root.as_mut_ptr(), atomic::Ordering::SeqCst);
                    subtree_root.set_parent(None);
                }
                sibling = Node::sibling(node); // should be parent.right
            } else if !node_is_left_child && sibling.left().is_black() {
                sibling.right().set_black();
                sibling.set_red();
                if let Some(subtree_root) = Self::rotate_left(sibling.unwrap())
                    && subtree_root.parent().is_none()
                {
                    root.store(subtree_root.as_mut_ptr(), atomic::Ordering::SeqCst);
                    subtree_root.set_parent(None);
                }
                sibling = Node::sibling(node); // should be parent.left
            }

            // Fall through to case 6

            // Case 6: Black sibling with at least one red child + "outer nephew" is red
            // Recolor sibling + parent + sibling's child, rotate around parent
            sibling.set_color(node.parent().color());
            node.parent().set_black();
            if node_is_left_child {
                sibling.right().set_black();
                if let Some(subtree_root) = Self::rotate_left(node.parent().unwrap())
                    && subtree_root.parent().is_none()
                {
                    root.store(subtree_root.as_mut_ptr(), atomic::Ordering::SeqCst);
                    subtree_root.set_parent(None);
                }
            } else {
                sibling.left().set_black();
                if let Some(subtree_root) = Self::rotate_right(node.parent().unwrap())
                    && subtree_root.parent().is_none()
                {
                    root.store(subtree_root.as_mut_ptr(), atomic::Ordering::SeqCst);
                    subtree_root.set_parent(None);
                }
            }
        }
    }
}

impl<'a, D> Rbt<'a, D>
where
    D: SliceKey + Copy + 'a,
{
    /// Replaces the memory of the tree with a new slice, copying the data from the old slice to the new slice.
    pub fn resize(&mut self, slice: &'a mut [u8]) {
        let root = (!self.root.load(atomic::Ordering::SeqCst).is_null())
            .then(|| self.storage.idx(self.root.load(atomic::Ordering::SeqCst)));

        self.storage.resize(slice);

        if let Some(idx) = root {
            self.root.store(self.storage.get_mut(idx).expect("Pointer Exists."), atomic::Ordering::SeqCst);
        }
    }

    #[cfg(feature = "alloc")]
    #[cfg_attr(docsrs, doc(cfg(feature = "alloc")))]
    #[allow(dead_code)]
    /// Performs a depth-first search on the tree, returning the ordered values.
    pub fn dfs(&self) -> alloc::vec::Vec<D> {
        let mut values = alloc::vec::Vec::new();
        Self::_dfs(self.root(), &mut values);
        values
    }

    #[cfg(feature = "alloc")]
    #[cfg_attr(docsrs, doc(cfg(feature = "alloc")))]
    #[allow(dead_code)]
    fn _dfs(node: Option<&Node<D>>, values: &mut alloc::vec::Vec<D>) {
        if let Some(node) = node {
            Self::_dfs(node.left(), values);
            values.push(node.data);
            Self::_dfs(node.right(), values);
        }
    }
}

impl<D> Default for Rbt<'_, D>
where
    D: SliceKey,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<D> core::fmt::Debug for Rbt<'_, D>
where
    D: SliceKey,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Rbt")
            .field("capacity", &self.capacity())
            .field("len", &self.len())
            .field("height", &self.height())
            .finish()
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    extern crate std;

    use super::*;
    use crate::node_size;

    use core::{
        ptr::null_mut,
        sync::atomic::{AtomicPtr, Ordering},
    };

    const RBT_MAX_SIZE: usize = 0x1000;

    #[test]
    fn simple_test() {
        let mut mem = [0; RBT_MAX_SIZE * node_size::<i32>()];
        let mut rbt: Rbt<i32> = Rbt::with_capacity(&mut mem);

        assert!(rbt.first().is_none());
        assert!(rbt.first_idx().is_none());
        assert!(rbt.last().is_none());
        assert!(rbt.last_idx().is_none());
        assert!(rbt.next(0).is_none());
        assert!(rbt.prev(0).is_none());

        assert!(rbt.add(5).is_ok());
        assert_eq!(rbt.storage.len(), 1);
        assert!(rbt.add(3).is_ok());
        assert!(rbt.add(7).is_ok());
        assert!(rbt.add(2).is_ok());
        assert!(rbt.add(6).is_ok());
        assert!(rbt.add(8).is_ok());
        assert!(rbt.add(9).is_ok());
        assert!(rbt.add(10).is_ok());
        assert_eq!(rbt.storage.len(), 8);
        assert!(rbt.add(10).is_err()); // Can't add the same value twice

        let values = rbt.dfs();
        assert_eq!(values, [2, 3, 5, 6, 7, 8, 9, 10]);
    }

    #[test]
    fn test_add_case_3() {
        /* Update colors when parent and uncle nodes are red.
            [17B]                  [17B]
             /  \                  /   \
          [09B] [19B] -------->  [09B] [19R] <- Updated
                /   \                   /  \
              [18R] [75R]  Updated -> [18B] [75B] <- Updated
                      \                       \
                      [81R]                  [81R]
        */
        let mut mem = [0; RBT_MAX_SIZE * node_size::<i32>()];
        let mut rbt: Rbt<i32> = Rbt::with_capacity(&mut mem);
        rbt.add(17).unwrap();

        // Root should be black
        {
            let root = rbt.root().unwrap();
            assert!(root.is_black());
        }

        // Add a node to the right, should be red
        rbt.add(19).unwrap();
        {
            let root = rbt.root().unwrap();
            assert!(root.is_black());
            let right = root.right().unwrap();
            assert!(right.is_red());
        }

        // Ensure no red-reds
        rbt.add(9).unwrap();
        rbt.add(18).unwrap();
        rbt.add(75).unwrap();
        {
            let root = rbt.root().unwrap();
            assert!(root.is_black());
            let right = root.right().unwrap();
            assert!(right.is_black());
            let right_l = right.left().unwrap();
            assert!(right_l.is_red());
            let right_r = right.right().unwrap();
            assert!(right_r.is_red());
        }

        // Adding a node off of 75 should cause a color change
        rbt.add(81).unwrap();
        {
            let root = rbt.root().unwrap();
            assert!(root.is_black());
            let right = root.right().unwrap();
            assert!(right.is_red());
            let right_l = right.left().unwrap();
            assert!(right_l.is_black());
            let right_r = right.right().unwrap();
            assert!(right_r.is_black());
            let right_r_r = right_r.right().unwrap();
            assert!(right_r_r.is_red());
        }
    }

    #[test]
    fn test_add_case_4() {
        /* Parent Node is red, uncle node is black, added node is Inner
           grandchild should cause a rotation.

          Final Expected State:
                   [17B]
                   /   \
                [09B] [24B]
                      /   \
                    [19R] [75R]
        */
        let mut mem = [0; RBT_MAX_SIZE * node_size::<i32>()];
        let mut rbt: Rbt<i32> = Rbt::with_capacity(&mut mem);
        rbt.add(17).unwrap();
        rbt.add(9).unwrap();
        rbt.add(19).unwrap();
        rbt.add(75).unwrap();
        rbt.add(24).unwrap();

        // Validate root (17)
        let root = rbt.root().unwrap();
        assert!(root.is_black());

        // Validate left child (9)
        let left = root.left().unwrap();
        assert!(left.is_black());
        assert_eq!(left.data, 9);
        assert_eq!(left.parent_ptr(), root.as_mut_ptr());

        // Validate right child(24)
        let right = root.right().unwrap();
        assert!(right.is_black());
        assert_eq!(right.data, 24);
        assert_eq!(right.parent_ptr(), root.as_mut_ptr());

        // Validate right child's left child (19)
        let right_l = right.left().unwrap();
        assert!(right_l.is_red());
        assert_eq!(right_l.data, 19);
        assert_eq!(right_l.parent_ptr(), right.as_mut_ptr());

        // Validate right child's right child (75)
        let right_r = right.right().unwrap();
        assert!(right_r.is_red());
        assert_eq!(right_r.data, 75);
    }

    #[test]
    fn test_rotate_right() {
        /* Verifies that the rotate right function works as expected.
             [50]              [75]
             /  \              /  \
           [10][75]    <--   [50][85]
               /  \          /  \
             [70][85]      [10][70]
        */
        let node = &Node::new(75);
        let left = &Node::new(50);
        let right = &Node::new(85);
        let left_l = &Node::new(10);
        let left_r = &Node::new(70);

        left.set_left(Some(left_l));
        left_l.set_parent(Some(left));
        left.set_right(Some(left_r));
        left_r.set_parent(Some(left));
        node.set_left(Some(left));
        left.set_parent(Some(node));
        node.set_right(Some(right));
        right.set_parent(Some(node));

        Rbt::<i32>::rotate_right(node);

        // Check left[50] <-> left_l[10] connection
        assert_eq!(left.left().unwrap().as_mut_ptr(), left_l.as_mut_ptr());
        assert_eq!(left_l.parent().unwrap().as_mut_ptr(), left.as_mut_ptr());

        // check left[50] <-> left_r[70] connection
        assert_eq!(left.right().unwrap().as_mut_ptr(), node.as_mut_ptr());
        assert_eq!(node.parent().unwrap().as_mut_ptr(), left.as_mut_ptr());

        // check left_l[10] has no children
        assert!(left_l.left().is_none());
        assert!(left_l.right().is_none());

        // check node[75] <-> left_r[70] connection
        assert_eq!(node.left().unwrap().as_mut_ptr(), left_r.as_mut_ptr());
        assert_eq!(left_r.parent().unwrap().as_mut_ptr(), node.as_mut_ptr());

        // check node[75] <-> right[85] connection
        assert_eq!(node.right().unwrap().as_mut_ptr(), right.as_mut_ptr());
        assert_eq!(right.parent().unwrap().as_mut_ptr(), node.as_mut_ptr());

        // Check right_r[70] has no children
        assert!(left_r.left().is_none());
        assert!(left_r.right().is_none());

        // Check right[85] has no children
        assert!(right.left().is_none());
        assert!(right.right().is_none());
    }

    #[test]
    fn test_rotate_left() {
        /* Verifies that the rotate left function works as expected.
             [50]              [75]
             /  \              /  \
           [10][75]    -->   [50][85]
               /  \          /  \
             [70][85]      [10][70]
        */
        let node = &Node::new(50);
        let left = &Node::new(10);
        let right = &Node::new(75);
        let right_l = &Node::new(70);
        let right_r = &Node::new(85);

        right.set_left(Some(right_l));
        right_l.set_parent(Some(right));
        right.set_right(Some(right_r));
        right_r.set_parent(Some(right));
        node.set_left(Some(left));
        left.set_parent(Some(node));
        node.set_right(Some(right));
        right.set_parent(Some(node));

        Rbt::<i32>::rotate_left(node);

        // Check right[75] <-left-> node[50] connection
        assert_eq!(right.left().unwrap().as_mut_ptr(), node.as_mut_ptr());
        assert_eq!(node.parent().unwrap().as_mut_ptr(), right.as_mut_ptr());

        // Check right[75] <-right-> right_r[85] connection
        assert_eq!(right.right().unwrap().as_mut_ptr(), right_r.as_mut_ptr());
        assert_eq!(right_r.parent().unwrap().as_mut_ptr(), right.as_mut_ptr());

        // Check node[50] <-left-> left[10] connection
        assert_eq!(node.left().unwrap().as_mut_ptr(), left.as_mut_ptr());
        assert_eq!(left.parent().unwrap().as_mut_ptr(), node.as_mut_ptr());

        // Check node[50] <-right-> right_l[70] connection
        assert_eq!(node.right().unwrap().as_mut_ptr(), right_l.as_mut_ptr());
        assert_eq!(right_l.parent().unwrap().as_mut_ptr(), node.as_mut_ptr());

        // Check left[10] has no children
        assert!(left.left().is_none());
        assert!(left.right().is_none());

        // Check right_r[85] has no children
        assert!(right_r.left().is_none());
        assert!(right_r.right().is_none());

        // Check right_l[70] has no children
        assert!(right_l.left().is_none());
        assert!(right_l.right().is_none());
    }

    #[test]
    fn test_delete_from_storage() {
        let mut mem = [0; 10 * node_size::<i32>()];
        let mut rbt = Rbt::<i32>::with_capacity(&mut mem);
        rbt.add(5).unwrap();
        rbt.add(3).unwrap();
        assert_eq!(rbt.storage.len(), 2);
        rbt.delete(&5).unwrap();
        assert_eq!(rbt.storage.len(), 1);
        rbt.delete(&3).unwrap();
        assert_eq!(rbt.storage.len(), 0);
    }

    #[test]
    fn test_delete_simple() {
        /* Verifies that deleting a node with a single child or no child works as expected.
                [50]      [50]
                /          /
              [10]   ->  [05]   ->   [50]
               /
             [05]
        */
        let node = &Node::new(50);
        let left = &Node::new(10);
        let left_l = &Node::new(5);

        node.set_left(Some(left));
        left.set_parent(Some(node));
        left.set_left(Some(left_l));
        left_l.set_parent(Some(left));

        // Delete a node with a single child.
        Rbt::<i32>::remove_node_with_zero_or_one_child(left);
        assert_eq!(node.left().as_mut_ptr(), left_l.as_mut_ptr());

        // Delete a node with no children.
        Rbt::<i32>::remove_node_with_zero_or_one_child(left_l);
        assert_eq!(node.left().as_mut_ptr(), null_mut());
        assert!(Rbt::<i32>::remove_node_with_zero_or_one_child(left_l).is_none());
    }

    #[test]
    fn test_delete_sibling_of_red() {
        /* Delete 09B
               [17B]                [19B]
               /   \                /   \
            [09B] [19R]       -> [17B] [75B]
                  /   \             \
               [18B] [75B]         [18R]
        */

        let root = &Node::new(17);
        root.set_black();

        let left = &Node::new(9);
        left.set_black();

        let right = &Node::new(19);
        right.set_red();

        let right_l = &Node::new(18);
        right_l.set_black();

        let right_r = &Node::new(75);
        right_r.set_black();

        root.set_left(Some(left));
        left.set_parent(Some(root));

        root.set_right(Some(right));
        right.set_parent(Some(root));

        right.set_left(Some(right_l));
        right_l.set_parent(Some(right));

        right.set_right(Some(right_r));
        right_r.set_parent(Some(right));

        let root_ptr = AtomicPtr::new(root.as_mut_ptr());
        Rbt::<i32>::remove_node_from_tree(&root_ptr, left);

        let new_root = unsafe { &*root_ptr.load(Ordering::SeqCst) };

        // Validate the new root
        assert_eq!(new_root.as_mut_ptr(), right.as_mut_ptr());
        assert_eq!(right.data, 19);
        assert!(right.is_black());
        assert!(right.parent().is_none());
        assert_eq!(right.left_ptr(), root.as_mut_ptr());
        assert_eq!(right.right_ptr(), right_r.as_mut_ptr());

        //Validate the left child
        assert_eq!(root.parent_ptr(), right.as_mut_ptr());
        assert_eq!(root.data, 17);
        assert!(root.is_black());
        assert!(root.left().is_none());
        assert_eq!(root.right_ptr(), right_l.as_mut_ptr());

        // Validate the right child
        assert_eq!(right_r.parent_ptr(), right.as_mut_ptr());
        assert_eq!(right_r.data, 75);
        assert!(right_r.is_black());
        assert!(right_r.left().is_none());
        assert!(right_r.right().is_none());

        // validate the right child of the left child
        assert_eq!(right_l.parent_ptr(), root.as_mut_ptr());
        assert_eq!(right_l.data, 18);
        assert!(right_l.is_red());
        assert!(right_l.left().is_none());
        assert!(right_l.right().is_none());
    }

    #[test]
    fn test_delete_sibling_black_with_red_parent() {
        /* Delete 75B
                  [17B]                   [17B]
                 /    \                  /   \
             [09B]     [19R]    ->   [09B]    [19B]
             /   \     /   \         /   \     /
           [03R][12R][18B][75B]    [03R][12R][18R]
        */

        let root = &Node::new(17);
        root.set_black();

        let left = &Node::new(9);
        left.set_black();

        let right = &Node::new(19);
        right.set_red();

        let left_l = &Node::new(3);
        left_l.set_red();

        let left_r = &Node::new(12);
        left_r.set_red();

        let right_l = &Node::new(18);
        right_l.set_black();

        let right_r = &Node::new(75);
        right_r.set_black();

        root.set_left(Some(left));
        left.set_parent(Some(root));

        root.set_right(Some(right));
        right.set_parent(Some(root));

        left.set_left(Some(left_l));
        left_l.set_parent(Some(left));

        left.set_right(Some(left_r));
        left_r.set_parent(Some(left));

        right.set_left(Some(right_l));
        right_l.set_parent(Some(right));

        right.set_right(Some(right_r));
        right_r.set_parent(Some(right));

        let root_ptr = AtomicPtr::new(root.as_mut_ptr());

        Rbt::<i32>::remove_node_from_tree(&root_ptr, right_r);

        let new_root = unsafe { &*root_ptr.load(Ordering::SeqCst) };
        assert_eq!(new_root.as_mut_ptr(), root.as_mut_ptr());
        assert_eq!(new_root.data, 17);
        assert!(new_root.is_black());
        assert!(new_root.parent().is_none());
        assert_eq!(new_root.left_ptr(), left.as_mut_ptr());
        assert_eq!(new_root.right_ptr(), right.as_mut_ptr());

        assert_eq!(left.parent_ptr(), new_root.as_mut_ptr());
        assert_eq!(left.data, 9);
        assert!(left.is_black());
        assert_eq!(left.left_ptr(), left_l.as_mut_ptr());
        assert_eq!(left.right_ptr(), left_r.as_mut_ptr());

        assert_eq!(left_l.parent_ptr(), left.as_mut_ptr());
        assert_eq!(left_l.data, 3);
        assert!(left_l.is_red());
        assert!(left_l.left().is_none());
        assert!(left_l.right().is_none());

        assert_eq!(left_r.parent_ptr(), left.as_mut_ptr());
        assert_eq!(left_r.data, 12);
        assert!(left_r.is_red());
        assert!(left_r.left().is_none());
        assert!(left_r.right().is_none());

        assert_eq!(right.parent_ptr(), new_root.as_mut_ptr());
        assert_eq!(right.data, 19);
        assert!(right.is_black());
        assert_eq!(right.left_ptr(), right_l.as_mut_ptr());
        assert!(right.right().is_none());

        assert_eq!(right_l.parent_ptr(), right.as_mut_ptr());
        assert_eq!(right_l.data, 18);
        assert!(right_l.is_red());
        assert!(right_l.left().is_none());
        assert!(right_l.right().is_none());
    }

    #[test]
    fn test_delete_sibling_black_with_black_parent() {
        /* Delete 18B
                  [17B]                   [17B]
                 /    \                  /   \
             [09B]     [19B]    ->   [09R]    [19B]
             /   \     /   \         /   \        \
           [03B][12B][18B][75B]    [03B][12B]    [75R]
        */

        let root = &Node::new(17);
        root.set_black();

        let left = &Node::new(9);
        left.set_black();

        let right = &Node::new(19);
        right.set_black();

        let left_l = &Node::new(3);
        left_l.set_black();

        let left_r = &Node::new(12);
        left_r.set_black();

        let right_l = &Node::new(18);
        right_l.set_black();

        let right_r = &Node::new(75);
        right_r.set_black();

        root.set_left(Some(left));
        left.set_parent(Some(root));

        root.set_right(Some(right));
        right.set_parent(Some(root));

        left.set_left(Some(left_l));
        left_l.set_parent(Some(left));

        left.set_right(Some(left_r));
        left_r.set_parent(Some(left));

        right.set_left(Some(right_l));
        right_l.set_parent(Some(right));

        right.set_right(Some(right_r));
        right_r.set_parent(Some(right));

        let root_ptr = AtomicPtr::new(root.as_mut_ptr());

        Rbt::<i32>::remove_node_from_tree(&root_ptr, right_l);

        let new_root = unsafe { &*root_ptr.load(Ordering::SeqCst) };
        assert_eq!(new_root.as_mut_ptr(), root.as_mut_ptr());
        assert_eq!(new_root.data, 17);
        assert!(new_root.is_black());
        assert!(new_root.parent().is_none());
        assert_eq!(new_root.left_ptr(), left.as_mut_ptr());
        assert_eq!(new_root.right_ptr(), right.as_mut_ptr());

        assert_eq!(left.parent_ptr(), new_root.as_mut_ptr());
        assert_eq!(left.data, 9);
        assert!(left.is_red());
        assert_eq!(left.left_ptr(), left_l.as_mut_ptr());
        assert_eq!(left.right_ptr(), left_r.as_mut_ptr());

        assert_eq!(left_l.parent_ptr(), left.as_mut_ptr());
        assert_eq!(left_l.data, 3);
        assert!(left_l.is_black());
        assert!(left_l.left().is_none());
        assert!(left_l.right().is_none());

        assert_eq!(left_r.parent_ptr(), left.as_mut_ptr());
        assert_eq!(left_r.data, 12);
        assert!(left_r.is_black());
        assert!(left_r.left().is_none());
        assert!(left_r.right().is_none());

        assert_eq!(right.parent_ptr(), new_root.as_mut_ptr());
        assert_eq!(right.data, 19);
        assert!(right.is_black());
        assert!(right.left().is_none());
        assert_eq!(right.right_ptr(), right_r.as_mut_ptr());

        assert_eq!(right_r.parent_ptr(), right.as_mut_ptr());
        assert_eq!(right_r.data, 75);
        assert!(right_r.is_red());
        assert!(right_r.left().is_none());
        assert!(right_r.right().is_none());
    }

    #[test]
    fn test_delete_sibling_black_with_red_child_and_black_outer_nephew() {
        /* Delete 18B
           [17B]            [17B]
           /   \            /   \
        [09B][19R]    ->  [09B][24R]
             /   \             /   \
           [18B][75B]       [19B] [75B]
                /
             [24R]
        */
        let root = &Node::new(17);
        root.set_black();

        let left = &Node::new(9);
        left.set_black();

        let right = &Node::new(19);
        right.set_red();

        let right_l = &Node::new(18);
        right_l.set_black();

        let right_r = &Node::new(75);
        right_r.set_black();

        let right_r_l = &Node::new(24);
        right_r_l.set_red();

        root.set_left(Some(left));
        left.set_parent(Some(root));

        root.set_right(Some(right));
        right.set_parent(Some(root));

        right.set_left(Some(right_l));
        right_l.set_parent(Some(right));

        right.set_right(Some(right_r));
        right_r.set_parent(Some(right));

        right_r.set_left(Some(right_r_l));
        right_r_l.set_parent(Some(right_r));

        let root_ptr = AtomicPtr::new(root.as_mut_ptr());
        Rbt::<i32>::remove_node_from_tree(&root_ptr, right_l);

        let new_root = unsafe { &*root_ptr.load(Ordering::SeqCst) };

        assert_eq!(new_root.as_mut_ptr(), root.as_mut_ptr());
        assert_eq!(new_root.data, 17);
        assert!(new_root.is_black());
        assert!(new_root.parent().is_none());
        assert_eq!(new_root.left_ptr(), left.as_mut_ptr());
        assert_eq!(new_root.right_ptr(), right_r_l.as_mut_ptr());

        assert_eq!(left.parent_ptr(), new_root.as_mut_ptr());
        assert_eq!(left.data, 9);
        assert!(left.is_black());
        assert!(left.left().is_none());
        assert!(left.right().is_none());

        assert_eq!(right_r_l.parent_ptr(), new_root.as_mut_ptr());
        assert_eq!(right_r_l.data, 24);
        assert!(right_r_l.is_red());
        assert_eq!(right_r_l.left_ptr(), right.as_mut_ptr());
        assert_eq!(right_r_l.right_ptr(), right_r.as_mut_ptr());

        assert_eq!(right.parent_ptr(), right_r_l.as_mut_ptr());
        assert_eq!(right.data, 19);
        assert!(right.is_black());
        assert!(right.left().is_none());
        assert!(right.right().is_none());

        assert_eq!(right_r.parent_ptr(), right_r_l.as_mut_ptr());
        assert_eq!(right_r.data, 75);
        assert!(right_r.is_black());
        assert!(right_r.left().is_none());
        assert!(right_r.right().is_none());
    }

    #[test]
    fn test_delete_sibling_black_with_red_child_and_red_outer_nephew() {
        /* Delete 18B
           [17B]            [17B]
           /   \            /   \
        [09B][19R]    ->  [09B][75R]
             /   \             /   \
           [18B][75B]       [19B] [81B]
                /  \            \
             [24R][81R]        [24R]
        */
        let root = &Node::new(17);
        root.set_black();

        let left = &Node::new(9);
        left.set_black();

        let right = &Node::new(19);
        right.set_red();

        let right_l = &Node::new(18);
        right_l.set_black();

        let right_r = &Node::new(75);
        right_r.set_black();

        let right_r_l = &Node::new(24);
        right_r_l.set_red();

        let right_r_r = &Node::new(81);
        right_r_r.set_red();

        root.set_left(Some(left));
        left.set_parent(Some(root));

        root.set_right(Some(right));
        right.set_parent(Some(root));

        right.set_left(Some(right_l));
        right_l.set_parent(Some(right));

        right.set_right(Some(right_r));
        right_r.set_parent(Some(right));

        right_r.set_left(Some(right_r_l));
        right_r_l.set_parent(Some(right_r));

        right_r.set_right(Some(right_r_r));
        right_r_r.set_parent(Some(right_r));

        let root_ptr = AtomicPtr::new(root.as_mut_ptr());
        Rbt::<i32>::remove_node_from_tree(&root_ptr, right_l);

        let new_root = unsafe { &*root_ptr.load(Ordering::SeqCst) };

        assert_eq!(new_root.as_mut_ptr(), root.as_mut_ptr());
        assert_eq!(new_root.data, 17);
        assert!(new_root.is_black());
        assert!(new_root.parent().is_none());
        assert_eq!(new_root.left_ptr(), left.as_mut_ptr());
        assert_eq!(new_root.right_ptr(), right_r.as_mut_ptr());

        assert_eq!(left.parent_ptr(), new_root.as_mut_ptr());
        assert_eq!(left.data, 9);
        assert!(left.is_black());
        assert!(left.left().is_none());
        assert!(left.right().is_none());

        assert_eq!(right_r.parent_ptr(), new_root.as_mut_ptr());
        assert_eq!(right_r.data, 75);
        assert!(right_r.is_red());
        assert_eq!(right_r.left_ptr(), right.as_mut_ptr());
        assert_eq!(right_r.right_ptr(), right_r_r.as_mut_ptr());

        assert_eq!(right.parent_ptr(), right_r.as_mut_ptr());
        assert_eq!(right.data, 19);
        assert!(right.is_black());
        assert!(right.left().is_none());
        assert_eq!(right.right_ptr(), right_r_l.as_mut_ptr());

        assert_eq!(right_r_r.parent_ptr(), right_r.as_mut_ptr());
        assert_eq!(right_r_r.data, 81);
        assert!(right_r_r.is_black());
        assert!(right_r_r.left().is_none());
        assert!(right_r_r.right().is_none());

        assert_eq!(right_r_l.parent_ptr(), right.as_mut_ptr());
        assert_eq!(right_r_l.data, 24);
        assert!(right_r_l.is_red());
        assert!(right_r_l.left().is_none());
        assert!(right_r_l.right().is_none());
    }

    #[test]
    fn test_add_many() {
        let mut mem = [0; RBT_MAX_SIZE * node_size::<usize>()];
        let mut rbt: Rbt<usize> = Rbt::with_capacity(&mut mem);
        assert!(rbt.add_many(0..RBT_MAX_SIZE).is_ok());
        assert_eq!(rbt.len(), RBT_MAX_SIZE);
    }

    #[test]
    fn test_get_functions() {
        #[derive(Debug)]
        struct MyType(usize, usize);
        impl crate::SliceKey for MyType {
            type Key = usize;
            fn key(&self) -> &Self::Key {
                &self.0
            }
        }

        let mut mem = [0; RBT_MAX_SIZE * node_size::<MyType>()];
        let mut rbt: Rbt<MyType> = Rbt::with_capacity(&mut mem);
        for i in 0..RBT_MAX_SIZE {
            assert!(rbt.add(MyType(i + 1, i)).is_ok());
        }

        for i in 0..RBT_MAX_SIZE {
            assert_eq!(rbt.get(&(i + 1)).unwrap().1, i);
        }
        assert!(rbt.get(&(RBT_MAX_SIZE + 1)).is_none());

        for i in 0..RBT_MAX_SIZE {
            let idx = rbt.get_idx(&(i + 1)).unwrap();
            unsafe { rbt.get_with_idx_mut(idx).unwrap().1 = i + 1 };
            assert_eq!(rbt.get_with_idx(idx).unwrap().1, i + 1);
        }
        unsafe {
            assert!(rbt.get_with_idx_mut(RBT_MAX_SIZE).is_none());
        }
        assert!(rbt.get_with_idx(RBT_MAX_SIZE).is_none());

        for i in 0..RBT_MAX_SIZE {
            unsafe { rbt.get_mut(&(i + 1)).unwrap().1 = i };
            assert_eq!(rbt.get(&(i + 1)).unwrap().1, i);
        }
        unsafe {
            assert!(rbt.get_mut(&(RBT_MAX_SIZE + 1)).is_none());
        }
    }

    #[test]
    fn test_get_closest1() {
        let mut mem = [0; 4096 * node_size::<i32>()];
        let mut rbt: Rbt<i32> = Rbt::with_capacity(&mut mem);
        assert_eq!(rbt.get_closest_idx(&1), None);

        let a = rbt.add(1).unwrap();
        let b = rbt.add(15).unwrap();
        let c = rbt.add(10).unwrap();
        let d = rbt.add(5).unwrap();

        assert_eq!(rbt.get_closest_idx(&1), Some(a));
        assert_eq!(rbt.get_closest_idx(&2), Some(a));
        assert_eq!(rbt.get_closest_idx(&5), Some(d));
        assert_eq!(rbt.get_closest_idx(&6), Some(d));
        assert_eq!(rbt.get_closest_idx(&10), Some(c));
        assert_eq!(rbt.get_closest_idx(&11), Some(c));
        assert_eq!(rbt.get_closest_idx(&15), Some(b));
        assert_eq!(rbt.get_closest_idx(&16), Some(b));
    }

    #[test]
    fn test_get_closest2() {
        let mut mem = [0; RBT_MAX_SIZE * node_size::<usize>()];
        let mut rbt: Rbt<usize> = Rbt::with_capacity(&mut mem);
        for i in 0..RBT_MAX_SIZE {
            assert!(rbt.add(i * 10).is_ok());
        }

        // Ensure that the closest index is always rounded down, no matter how close the value is to the next index
        for i in 1..RBT_MAX_SIZE {
            assert_eq!(rbt.get_closest_idx(&((i * 10) - 1)).unwrap(), i - 1);
            assert_eq!(rbt.get_closest_idx(&(i * 10)).unwrap(), i);
            assert_eq!(rbt.get_closest_idx(&((i * 10) + 1)).unwrap(), i);
        }
    }

    #[test]
    fn test_iteration() {
        let mut mem = [0; RBT_MAX_SIZE * node_size::<usize>()];
        let mut rbt: Rbt<usize> = Rbt::with_capacity(&mut mem);
        for i in 0..RBT_MAX_SIZE {
            assert!(rbt.add(i).is_ok());
        }

        let mut current = rbt.first();
        let mut val = 0;
        while let Some(cur) = current {
            assert_eq!(cur, &val);
            current = rbt.next(*cur);
            val += 1
        }

        val -= 1;
        let mut current = rbt.last();
        while let Some(cur) = current {
            assert_eq!(cur, &val);
            current = rbt.prev(*cur);
            val = val.saturating_sub(1);
        }

        let mut current = rbt.first_idx();
        while let Some(cur) = current {
            assert_eq!(rbt.get_with_idx(cur).unwrap(), &cur);
            current = rbt.next_idx(cur);
        }

        let mut current = rbt.first_idx();
        while let Some(cur) = current {
            assert_eq!(rbt.get_with_idx(cur).unwrap(), &cur);
            current = rbt.prev_idx(cur);
        }

        let mut current = rbt.first_idx();
        while let Some(cur) = current {
            assert!(rbt.delete_with_idx(cur).is_ok());
            current = rbt.first_idx();
        }
        assert_eq!(rbt.len(), 0);
    }

    #[test]
    fn test_simple_resize() {
        let mut rbt = Rbt::<usize>::new();

        let mut mem = [0; 20 * node_size::<usize>()];
        rbt.resize(&mut mem);

        for i in 0..10 {
            assert!(rbt.add(i).is_ok());
        }

        for i in 0..10 {
            assert_eq!(rbt.get(&i).unwrap(), &i);
        }
    }

    #[test]
    fn test_resize_with_existing_data() {
        let mut mem = [0; 10 * node_size::<usize>()];
        let mut rbt = Rbt::<usize>::with_capacity(&mut mem);

        assert_eq!(rbt.len(), 0);
        assert_eq!(rbt.capacity(), 10);

        for i in 0..10 {
            assert!(rbt.add(i).is_ok());
        }

        let mut new_mem = [0; 20 * node_size::<usize>()];
        rbt.resize(&mut new_mem);

        assert_eq!(rbt.len(), 10);
        assert_eq!(rbt.capacity(), 20);

        for i in 0..10 {
            assert_eq!(rbt.get(&i).unwrap(), &i);
        }

        for i in 10..20 {
            assert!(rbt.add(i).is_ok());
        }

        for i in 0..20 {
            assert_eq!(rbt.get(&i).unwrap(), &i);
        }
    }
}

#[cfg(test)]
mod fuzz_tests {
    extern crate std;
    use crate::{Rbt, node_size};
    use rand::{Rng, seq::SliceRandom};
    use std::{collections::HashSet, vec::Vec};

    const RBT_MAX_SIZE: usize = 0x1000;

    #[test]
    fn fuzz_add() {
        for _ in 0..100 {
            let mut mem = [0; RBT_MAX_SIZE * node_size::<u32>()];
            let mut rbt: Rbt<u32> = Rbt::with_capacity(&mut mem);
            let mut rng = rand::thread_rng();
            let min = 1;
            let max = 100_000;

            let mut random_numbers = HashSet::new();

            while random_numbers.len() < RBT_MAX_SIZE - 1 {
                let num = rng.gen_range(min..=max);
                random_numbers.insert(num);
            }

            let mut random_numbers: Vec<_> = random_numbers.into_iter().collect();
            random_numbers.shuffle(&mut rng);

            assert_eq!(random_numbers.len(), RBT_MAX_SIZE - 1);
            for num in random_numbers.iter() {
                assert!(rbt.add(*num).is_ok());
            }
            assert!(rbt.height() < 25);
            random_numbers.sort();

            let ordered_numbers = rbt.dfs();
            assert_eq!(ordered_numbers, random_numbers);
        }
    }

    #[test]
    fn fuzz_delete() {
        for _ in 0..100 {
            let mut mem = [0; RBT_MAX_SIZE * node_size::<u32>()];
            let mut rbt: Rbt<u32> = Rbt::with_capacity(&mut mem);
            let mut rng = rand::thread_rng();
            let min = 1;
            let max = 100_000;

            let mut random_numbers = HashSet::new();
            while random_numbers.len() < RBT_MAX_SIZE {
                let num = rng.gen_range(min..=max);
                random_numbers.insert(num);
            }

            let mut random_numbers: Vec<_> = random_numbers.into_iter().collect();
            random_numbers.shuffle(&mut rng);

            assert_eq!(random_numbers.len(), RBT_MAX_SIZE);
            for num in random_numbers.iter() {
                assert!(rbt.add(*num).is_ok());
            }

            // Delete all the numbers
            random_numbers.shuffle(&mut rng);
            while let Some(num) = random_numbers.pop() {
                assert!(rbt.delete(&num).is_ok());
            }
            assert_eq!(rbt.len(), 0);
            assert!(rbt.root().is_none());
        }
    }

    #[test]
    fn fuzz_search() {
        let mut mem = [0; RBT_MAX_SIZE * node_size::<u32>()];
        let mut rbt: Rbt<u32> = Rbt::with_capacity(&mut mem);
        let mut rng = rand::thread_rng();
        let min = 1;
        let max = 100_000;

        let mut random_numbers = HashSet::new();
        while random_numbers.len() < RBT_MAX_SIZE {
            let num = rng.gen_range(min..=max);
            random_numbers.insert(num);
        }

        let mut random_numbers: Vec<_> = random_numbers.into_iter().collect();
        random_numbers.shuffle(&mut rng);

        assert_eq!(random_numbers.len(), RBT_MAX_SIZE);
        for num in random_numbers.iter() {
            assert!(rbt.add(*num).is_ok());
        }

        // Search for numbers that exist in the tree
        for _ in 0..100_000 {
            let num = random_numbers.choose(&mut rng).unwrap();
            assert!(rbt.get(num).is_some());
        }

        // Search for numbers that do not exist in the tree
        for _ in 0..100_000 {
            let to_search = rng.gen_bool(0.5);
            let random_number =
                if to_search { rng.gen_range(0..=min - 1) } else { rng.gen_range(max + 1..=max + 50_000) };
            assert!(rbt.get(&random_number).is_none());
        }
    }
}
