//! Slice Collections - Binary Search Tree (BST)
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
#[cfg(feature = "alloc")]
extern crate alloc;
use core::{
    cmp::Ordering,
    sync::atomic::{self, AtomicPtr},
};

use crate::{
    node::{Node, NodeTrait, Storage},
    Error, Result, SliceKey,
};

/// A binary search tree that can hold up to `SIZE` nodes.
pub struct Bst<'a, D>
where
    D: SliceKey,
{
    storage: Storage<'a, D>,
    root: AtomicPtr<Node<D>>,
}

impl<'a, D> Bst<'a, D>
where
    D: SliceKey + 'a,
{
    /// Creates a zero capacity red-black tree.
    ///
    /// This is useful for creating a tree at compile time and replacing the memory later. Use
    /// [with_capacity](Self::with_capacity) to create a tree with a given slice of memory immediately. Otherwise use
    /// [resize](Self::resize) to replace the memory later.
    pub const fn new() -> Self {
        Bst { storage: Storage::new(), root: AtomicPtr::new(core::ptr::null_mut()) }
    }

    /// Creates a new binary tree with a given slice of memory.
    pub fn with_capacity(slice: &'a mut [u8]) -> Self {
        Self { storage: Storage::with_capacity(slice), root: AtomicPtr::default() }
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

    /// Returns the current root of the tree.
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

        if self.root.load(atomic::Ordering::SeqCst).is_null() {
            self.root.store(node.as_mut_ptr(), atomic::Ordering::SeqCst);
            return Ok(idx);
        }

        let root = unsafe { &*self.root.load(atomic::Ordering::SeqCst) };
        Self::add_node(root, node)?;
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

    /// Adds a node into the tree. The node must already exist in the storage.
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
    /// Returns `Some(&D)` if the value was found.
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
    pub unsafe fn get_mut(&self, key: &D::Key) -> Option<&mut D> {
        match self.get_node(key) {
            Some(node) => Some(&mut (*node.as_mut_ptr()).data),
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
            match key.cmp(node.data.key()) {
                Ordering::Equal => return Some(node),
                Ordering::Less => current_idx = node.left(),
                Ordering::Greater => current_idx = node.right(),
            }
        }
        None
    }

    /// Deletes a value from the tree from the given key.
    ///
    /// Returns `Ok(())` if the value was found and deleted.
    ///
    /// Returns `Error::NotFound` if the value was not found.
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
        if to_delete.left().is_none() || to_delete.right().is_none() {
            let moved_up = Self::remove_node_with_zero_or_one_child(to_delete);
            if to_delete.parent().is_none() {
                root.store(moved_up.as_mut_ptr(), atomic::Ordering::SeqCst);
                moved_up.set_parent(None);
            }
        } else {
            let successor = Node::successor(to_delete).expect("to_delete has both children");
            Node::swap(to_delete, successor);
            if successor.parent().is_none() {
                root.store(successor.as_mut_ptr(), atomic::Ordering::SeqCst);
                successor.set_parent(None);
            }
            let _ = Self::remove_node_with_zero_or_one_child(to_delete);
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
        } else {
            parent.set_right(None);
        }
        None
    }
}

impl<D> Default for Bst<'_, D>
where
    D: SliceKey,
{
    fn default() -> Self {
        Self::new()
    }
}

/// Methods that require D to also be [Copy](core::marker::Copy).
impl<'a, D> Bst<'a, D>
where
    D: Copy + SliceKey + 'a,
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
impl<D> core::fmt::Debug for Bst<'_, D>
where
    D: SliceKey,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Bst")
            .field("capacity", &self.capacity())
            .field("len", &self.len())
            .field("height", &self.height())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use crate::{node_size, Bst};

    const BST_MAX_SIZE: usize = 4096;

    #[test]
    fn simple_test() {
        let mut mem = [0; BST_MAX_SIZE * node_size::<i32>()];
        let mut bst: Bst<i32> = Bst::with_capacity(&mut mem);

        assert!(bst.first().is_none());
        assert!(bst.first_idx().is_none());
        assert!(bst.last().is_none());
        assert!(bst.last_idx().is_none());
        assert!(bst.next(0).is_none());
        assert!(bst.prev(0).is_none());

        assert!(bst.add(5).is_ok());
        assert_eq!(bst.storage.len(), 1);
        assert!(bst.add(3).is_ok());
        assert!(bst.add(7).is_ok());
        assert!(bst.add(2).is_ok());
        assert!(bst.add(6).is_ok());
        assert!(bst.add(8).is_ok());
        assert!(bst.add(9).is_ok());
        assert!(bst.add(10).is_ok());
        assert_eq!(bst.storage.len(), 8);
        assert!(bst.add(10).is_err()); // Can't add the same value twice

        let values = bst.dfs();
        assert_eq!(values, [2, 3, 5, 6, 7, 8, 9, 10]);
    }

    #[test]
    fn test_add_many() {
        let mut mem = [0; BST_MAX_SIZE * node_size::<usize>()];
        let mut bst: Bst<usize> = Bst::with_capacity(&mut mem);
        assert!(bst.add_many(0..BST_MAX_SIZE).is_ok());
        assert_eq!(bst.len(), BST_MAX_SIZE);
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

        let mut mem = [0; BST_MAX_SIZE * node_size::<MyType>()];
        let mut bst: Bst<MyType> = Bst::with_capacity(&mut mem);
        for i in 0..BST_MAX_SIZE {
            assert!(bst.add(MyType(i + 1, i)).is_ok());
        }

        for i in 0..BST_MAX_SIZE {
            assert_eq!(bst.get(&(i + 1)).unwrap().1, i);
        }
        assert!(bst.get(&(BST_MAX_SIZE + 1)).is_none());

        for i in 0..BST_MAX_SIZE {
            let idx = bst.get_idx(&(i + 1)).unwrap();
            unsafe { bst.get_with_idx_mut(idx).unwrap().1 = i + 1 };
            assert_eq!(bst.get_with_idx(idx).unwrap().1, i + 1);
        }
        unsafe {
            assert!(bst.get_with_idx_mut(BST_MAX_SIZE).is_none());
        }
        assert!(bst.get_with_idx(BST_MAX_SIZE).is_none());

        for i in 0..BST_MAX_SIZE {
            unsafe { bst.get_mut(&(i + 1)).unwrap().1 = i };
            assert_eq!(bst.get(&(i + 1)).unwrap().1, i);
        }
        unsafe {
            assert!(bst.get_mut(&(BST_MAX_SIZE + 1)).is_none());
        }
    }
    #[test]
    fn test_find_closest1() {
        let mut mem = [0; BST_MAX_SIZE * node_size::<i32>()];
        let mut bst: Bst<i32> = Bst::with_capacity(&mut mem);
        assert_eq!(bst.get_closest_idx(&1), None);

        let a = bst.add(1).unwrap();
        let b = bst.add(15).unwrap();
        let c = bst.add(10).unwrap();
        let d = bst.add(5).unwrap();

        assert_eq!(bst.get_closest_idx(&1), Some(a));
        assert_eq!(bst.get_closest_idx(&2), Some(a));
        assert_eq!(bst.get_closest_idx(&5), Some(d));
        assert_eq!(bst.get_closest_idx(&6), Some(d));
        assert_eq!(bst.get_closest_idx(&10), Some(c));
        assert_eq!(bst.get_closest_idx(&11), Some(c));
        assert_eq!(bst.get_closest_idx(&15), Some(b));
        assert_eq!(bst.get_closest_idx(&16), Some(b));
    }

    #[test]
    fn test_get_closest2() {
        let mut mem = [0; BST_MAX_SIZE * node_size::<usize>()];
        let mut bst: Bst<usize> = Bst::with_capacity(&mut mem);
        for i in 0..BST_MAX_SIZE {
            assert!(bst.add(i * 10).is_ok());
        }

        // Ensure that the closest index is always rounded down, no matter how close the value is to the next index
        for i in 1..BST_MAX_SIZE {
            assert_eq!(bst.get_closest_idx(&((i * 10) - 1)).unwrap(), i - 1);
            assert_eq!(bst.get_closest_idx(&(i * 10)).unwrap(), i);
            assert_eq!(bst.get_closest_idx(&((i * 10) + 1)).unwrap(), i);
        }
    }

    #[test]
    fn test_iteration() {
        let mut mem = [0; BST_MAX_SIZE * node_size::<usize>()];
        let mut bst: Bst<usize> = Bst::with_capacity(&mut mem);
        for i in 0..BST_MAX_SIZE {
            assert!(bst.add(i).is_ok());
        }

        let mut current = bst.first();
        let mut val = 0;
        while let Some(cur) = current {
            assert_eq!(cur, &val);
            current = bst.next(*cur);
            val += 1
        }

        val -= 1;
        let mut current = bst.last();
        while let Some(cur) = current {
            assert_eq!(cur, &val);
            current = bst.prev(*cur);
            val = val.saturating_sub(1);
        }

        let mut current = bst.first_idx();
        while let Some(cur) = current {
            assert_eq!(bst.get_with_idx(cur).unwrap(), &cur);
            current = bst.next_idx(cur);
        }

        let mut current = bst.first_idx();
        while let Some(cur) = current {
            assert_eq!(bst.get_with_idx(cur).unwrap(), &cur);
            current = bst.prev_idx(cur);
        }

        let mut current = bst.first_idx();
        while let Some(cur) = current {
            assert!(bst.delete_with_idx(cur).is_ok());
            current = bst.first_idx();
        }
        assert_eq!(bst.len(), 0);
    }

    #[test]
    fn test_simple_resize() {
        let mut bst = Bst::<usize>::new();

        let mut mem = [0; 20 * node_size::<usize>()];
        bst.resize(&mut mem);

        for i in 0..10 {
            assert!(bst.add(i).is_ok());
        }

        for i in 0..10 {
            assert_eq!(bst.get(&i).unwrap(), &i);
        }
    }

    #[test]
    fn test_resize_with_existing_data() {
        let mut mem = [0; 10 * node_size::<usize>()];
        let mut bst = Bst::<usize>::with_capacity(&mut mem);

        assert_eq!(bst.len(), 0);
        assert_eq!(bst.capacity(), 10);

        for i in 0..10 {
            assert!(bst.add(i).is_ok());
        }

        let mut new_mem = [0; 20 * node_size::<usize>()];
        bst.resize(&mut new_mem);

        assert_eq!(bst.len(), 10);
        assert_eq!(bst.capacity(), 20);

        for i in 0..10 {
            assert_eq!(bst.get(&i).unwrap(), &i);
        }

        for i in 10..20 {
            assert!(bst.add(i).is_ok());
        }

        for i in 0..20 {
            assert_eq!(bst.get(&i).unwrap(), &i);
        }
    }
}

#[cfg(test)]
mod fuzz_tests {
    extern crate std;
    use crate::{node_size, Bst};
    use rand::{seq::SliceRandom, Rng};
    use std::{collections::HashSet, vec::Vec};

    const BST_MAX_SIZE: usize = 4096;

    #[test]
    fn fuzz_add() {
        for _ in 0..100 {
            let mut mem = [0; BST_MAX_SIZE * node_size::<i32>()];
            let mut bst: Bst<i32> = Bst::with_capacity(&mut mem);
            let mut rng = rand::thread_rng();
            let min = 1;
            let max = 100_000;

            let mut random_numbers = HashSet::new();

            while random_numbers.len() < BST_MAX_SIZE {
                let num = rng.gen_range(min..=max);
                random_numbers.insert(num);
            }

            let mut random_numbers: Vec<_> = random_numbers.into_iter().collect();
            random_numbers.shuffle(&mut rng);

            assert_eq!(random_numbers.len(), BST_MAX_SIZE);
            for num in random_numbers.iter() {
                assert!(bst.add(*num).is_ok());
            }

            // Random inserts should not make the tree too unbalanced
            assert!(bst.height() < 50);
            random_numbers.sort();

            let ordered_numbers = bst.dfs();
            assert_eq!(ordered_numbers, random_numbers);
        }
    }

    #[test]
    fn fuzz_search() {
        let mut mem = [0; BST_MAX_SIZE * node_size::<i32>()];
        let mut bst: Bst<i32> = Bst::with_capacity(&mut mem);
        let mut rng = rand::thread_rng();
        let min = 50_000;
        let max = 100_000;

        let mut random_numbers = HashSet::new();
        while random_numbers.len() < BST_MAX_SIZE {
            let num = rng.gen_range(min..=max);
            random_numbers.insert(num);
        }

        let mut random_numbers: Vec<_> = random_numbers.into_iter().collect();
        random_numbers.shuffle(&mut rng);

        assert_eq!(random_numbers.len(), BST_MAX_SIZE);
        for num in random_numbers.iter() {
            assert!(bst.add(*num).is_ok());
        }

        // Search for numbers that exist in the tree
        for _ in 0..100_000 {
            let num = random_numbers.choose(&mut rng).unwrap();
            assert!(bst.get(num).is_some());
        }

        // Search for numbers that do not exist in the tree
        for _ in 0..100_000 {
            let to_search = rng.gen_bool(0.5);
            let random_number =
                if to_search { rng.gen_range(0..=min - 1) } else { rng.gen_range(max + 1..=max + 50_000) };
            assert!(bst.get(&random_number).is_none());
        }
    }

    #[test]
    fn fuzz_delete() {
        let mut mem = [0; BST_MAX_SIZE * node_size::<i32>()];
        let mut bst: Bst<i32> = Bst::with_capacity(&mut mem);
        let mut rng = rand::thread_rng();
        let min = 1;
        let max = 100_000;

        let mut random_numbers = HashSet::new();
        while random_numbers.len() < BST_MAX_SIZE {
            let num = rng.gen_range(min..=max);
            random_numbers.insert(num);
        }

        let mut random_numbers: Vec<_> = random_numbers.into_iter().collect();
        random_numbers.shuffle(&mut rng);

        assert_eq!(random_numbers.len(), BST_MAX_SIZE);
        for num in random_numbers.iter() {
            assert!(bst.add(*num).is_ok());
        }

        // Delete all the numbers
        random_numbers.shuffle(&mut rng);
        while let Some(num) = random_numbers.pop() {
            assert!(bst.delete(&num).is_ok());
        }

        assert_eq!(bst.storage.len(), 0);
    }
}
