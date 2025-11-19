//! Slice Collections - Node for a Red-Black Tree
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use core::{cell::Cell, mem, ptr::NonNull, slice};

use crate::{Error, Result, SliceKey};

/// The color RED of a node in a red-black tree.
pub const RED: bool = false;
/// The color BLACK of a node in a red-black tree.
pub const BLACK: bool = true;

/// Returns the size of a internal node in bytes, useful for calculating the slice size for the storage.
pub const fn node_size<D: SliceKey>() -> usize {
    core::mem::size_of::<Node<D>>()
}

/// A on-stack storage container for the nodes of a red-black tree.
pub(crate) struct Storage<'a, D>
where
    D: SliceKey,
{
    /// The storage container for the nodes.
    data: &'a mut [Node<D>],
    /// The number of nodes in the tree.
    length: usize,
    /// A linked list of free nodes in the storage container.
    available: Cell<*mut Node<D>>,
}

impl<'a, D> Storage<'a, D>
where
    D: SliceKey,
{
    /// Creates a empty, zero-capacity storage container.
    pub const fn new() -> Storage<'a, D> {
        let ptr = NonNull::<Node<D>>::dangling();
        Self {
            // SAFETY: Elements are dereferenced from the zero-length slice. Because the slice length is 0,
            // no invalid memory accesses occur.
            data: unsafe { slice::from_raw_parts_mut(ptr.as_ptr(), 0) },
            length: 0,
            available: Cell::new(core::ptr::null_mut()),
        }
    }

    /// Create a new storage container with a slice of memory.
    pub fn with_capacity(slice: &'a mut [u8]) -> Storage<'a, D> {
        let storage = Storage {
            // SAFETY: This is reinterpreting a byte slice as a Node<D> slice.
            // 1. The alignment is checked implicitly by the slice bounds.
            // 2. The correct number of Node<D> elements that fit in the byte slice is calculated.
            // 3. The lifetime ensures the byte slice remains valid for the storage's lifetime
            data: unsafe {
                slice::from_raw_parts_mut::<'a, Node<D>>(
                    slice as *mut [u8] as *mut Node<D>,
                    slice.len() / mem::size_of::<Node<D>>(),
                )
            },
            length: 0,
            available: Cell::default(),
        };

        Self::build_linked_list(storage.data);
        storage.available.set(storage.data[0].as_mut_ptr());
        storage
    }

    fn build_linked_list(buffer: &[Node<D>]) {
        let mut node = &buffer[0];
        for next in buffer.iter().skip(1) {
            node.set_right(Some(next));
            next.set_left(Some(node));
            node = next;
        }
    }

    /// Get the number of nodes in the storage container.
    pub fn len(&self) -> usize {
        self.length
    }

    /// Get the capacity of the storage container.
    pub fn capacity(&self) -> usize {
        self.data.len()
    }

    /// Add a new node to the storage container, returning a mutable reference to the node.
    ///
    /// # Time Complexity
    ///
    /// O(1)
    ///
    pub fn add(&mut self, data: D) -> Result<(usize, &mut Node<D>)> {
        let available_ptr = self.available.get();
        if !available_ptr.is_null() && self.length != self.capacity() {
            // SAFETY: available_ptr is checked to be non-null and points to a valid Node<D> in self.data.
            let node = unsafe { &mut *available_ptr };
            self.available.set(node.right_ptr());
            node.set_left(None);
            node.set_right(None);
            node.set_parent(None);
            node.data = data;
            self.length += 1;
            Ok((self.idx(node.as_mut_ptr()), node))
        } else {
            Err(Error::OutOfSpace)
        }
    }

    /// Delete a node from the storage container.
    ///
    /// # Time Complexity
    ///
    /// O(1)
    ///
    pub fn delete(&mut self, node: *mut Node<D>) {
        if node.is_null() {
            return;
        }
        // SAFETY: node is checked to be non-null and is expected to point to a valid Node<D>
        // that was previously allocated from this storage. The caller is responsible for
        // ensuring the pointer is valid.
        let node = unsafe { &mut *node };
        node.set_parent(None);
        node.set_left(None);
        let available_ptr = self.available.get();
        if !available_ptr.is_null() {
            // SAFETY: available_ptr is non-null and points to the head of our free list,
            // which contains valid Node<D> pointers from self.data.
            let root = unsafe { &mut *available_ptr };
            node.set_right(Some(root));
            root.set_left(Some(node));
        } else {
            node.set_right(None);
        }

        self.available.set(node.as_mut_ptr());
        self.length -= 1;
    }

    /// Get the index of a node in the storage container based off the pointer.
    pub fn idx(&self, ptr: *mut Node<D>) -> usize {
        debug_assert!(!ptr.is_null());
        // SAFETY: Meets the following requirements as specified in `offset_from`:
        // - `ptr` and `self.data.as_ptr()` are derived from the same allocation (the same slice).
        // - The distance between the pointers, in bytes, must be an exact multiple of the size of Node<T>.
        unsafe { ptr.offset_from(self.data.as_ptr()) as usize }
    }

    /// Gets a reference to a node in the storage container using an index
    ///
    /// # Time Complexity
    ///
    /// O(1)
    ///
    pub fn get(&self, index: usize) -> Option<&Node<D>> {
        self.data.get(index)
    }

    /// Gets a mutable reference to a node in the storage container using an index
    ///
    /// # Time Complexity
    ///
    /// O(1)
    ///
    pub fn get_mut(&mut self, index: usize) -> Option<&mut Node<D>> {
        self.data.get_mut(index)
    }
}

impl<'a, D> Storage<'a, D>
where
    D: SliceKey + Copy,
{
    /// Resizes the storage container to a new slice of memory.
    ///
    /// # Panics
    ///
    /// Panics if the new slice is smaller than the current length of the storage container.
    ///
    /// # Time Complexity
    ///
    /// O(n)
    pub fn resize(&mut self, slice: &'a mut [u8]) {
        // SAFETY: This is reinterpreting a byte slice as a Node<D> slice.
        // 1. The alignment is handled by slice casting rules
        // 2. The correct number of Node<D> elements that fit in the byte slice is calculated
        // 3. The lifetime 'a ensures the byte slice remains valid for the storage's lifetime
        let buffer = unsafe {
            slice::from_raw_parts_mut::<'a, Node<D>>(
                slice as *mut [u8] as *mut Node<D>,
                slice.len() / mem::size_of::<Node<D>>(),
            )
        };

        assert!(buffer.len() >= self.len());

        // When current capacity is 0, we just need to copy the data and build the available list
        if self.capacity() == 0 {
            self.data = buffer;
            Self::build_linked_list(self.data);
            self.available.set(self.data[0].as_mut_ptr());
            return;
        }

        // Copy the data from the old buffer to the new buffer. Update the pointers to the new buffer
        for i in 0..self.len() {
            let old = &self.data[i];

            buffer[i].data = old.data;
            buffer[i].set_color(old.color());

            if let Some(left) = old.left() {
                let idx = self.idx(left.as_mut_ptr());
                buffer[i].set_left(Some(&buffer[idx]));
            } else {
                buffer[i].set_left(None);
            }

            if let Some(right) = old.right() {
                let idx = self.idx(right.as_mut_ptr());
                buffer[i].set_right(Some(&buffer[idx]));
            } else {
                buffer[i].set_right(None);
            }

            if let Some(parent) = old.parent() {
                let idx = self.idx(parent.as_mut_ptr());
                buffer[i].set_parent(Some(&buffer[idx]));
            } else {
                buffer[i].set_parent(None);
            }
        }

        let idx = if !self.available.get().is_null() { self.idx(self.available.get()) } else { self.len() };

        Self::build_linked_list(&buffer[idx..]);
        self.available.set(buffer[idx].as_mut_ptr());

        self.data = buffer;
    }
}

pub(crate) trait NodeTrait<D>
where
    D: SliceKey,
{
    fn set_color(&self, color: bool);
    fn set_red(&self) {
        self.set_color(RED);
    }
    fn set_black(&self) {
        self.set_color(BLACK);
    }
    fn is_red(&self) -> bool;
    fn is_black(&self) -> bool;
    fn color(&self) -> bool;
    fn parent(&self) -> Option<&Node<D>>;
    // This trait function nor any of its implementations are used in the codebase, however the
    // pattern makes sense, and is kept for future possible use. If the implementation is ever
    // used, the #[allow(dead_code)] should be removed.
    #[allow(dead_code)]
    fn parent_ptr(&self) -> *mut Node<D>;
    fn set_parent(&self, node: Option<&Node<D>>);
    fn left(&self) -> Option<&Node<D>>;
    fn left_ptr(&self) -> *mut Node<D>;
    fn set_left(&self, node: Option<&Node<D>>);
    fn right(&self) -> Option<&Node<D>>;
    fn right_ptr(&self) -> *mut Node<D>;
    fn set_right(&self, node: Option<&Node<D>>);
    fn as_mut_ptr(&self) -> *mut Node<D>;
}

impl<D> NodeTrait<D> for Node<D>
where
    D: SliceKey,
{
    fn set_color(&self, color: bool) {
        self.color.set(color);
    }

    fn is_red(&self) -> bool {
        self.color.get() == RED
    }

    fn is_black(&self) -> bool {
        self.color.get() == BLACK
    }

    fn color(&self) -> bool {
        self.color.get()
    }

    fn parent(&self) -> Option<&Node<D>> {
        let node = self.parent.get();
        // SAFETY: If the pointer is not null, it points to a valid Node<D> in the storage.
        unsafe { node.as_ref() }
    }

    fn parent_ptr(&self) -> *mut Node<D> {
        self.parent.get()
    }

    fn set_parent(&self, node: Option<&Node<D>>) {
        match node {
            None => {
                self.parent.set(core::ptr::null_mut());
            }
            Some(node) => {
                self.parent.set(node.as_mut_ptr());
            }
        }
    }

    fn left(&self) -> Option<&Node<D>> {
        let node = self.left.get();
        // SAFETY: If the pointer is not null, it points to a valid Node<D> in the storage.
        unsafe { node.as_ref() }
    }

    fn left_ptr(&self) -> *mut Node<D> {
        self.left.get()
    }

    fn set_left(&self, node: Option<&Node<D>>) {
        match node {
            None => {
                self.left.set(core::ptr::null_mut());
            }
            Some(node) => {
                self.left.set(node.as_mut_ptr());
            }
        }
    }

    fn right(&self) -> Option<&Node<D>> {
        let node = self.right.get();
        // SAFETY: If the pointer is not null, it points to a valid Node<D> in the storage.
        unsafe { node.as_ref() }
    }

    fn right_ptr(&self) -> *mut Node<D> {
        self.right.get()
    }

    fn set_right(&self, node: Option<&Node<D>>) {
        match node {
            None => {
                self.right.set(core::ptr::null_mut());
            }
            Some(node) => {
                self.right.set(node.as_mut_ptr());
            }
        }
    }

    fn as_mut_ptr(&self) -> *mut Node<D> {
        self as *const _ as *mut _
    }
}

impl<D> NodeTrait<D> for Option<&Node<D>>
where
    D: SliceKey,
{
    fn set_color(&self, color: bool) {
        self.inspect(|n| n.set_color(color));
    }

    fn color(&self) -> bool {
        match self {
            Some(node) => node.color(),
            None => BLACK,
        }
    }

    fn is_red(&self) -> bool {
        match self {
            Some(node) => node.is_red(),
            None => false,
        }
    }

    fn is_black(&self) -> bool {
        match self {
            Some(node) => node.is_black(),
            None => true,
        }
    }

    fn parent(&self) -> Option<&Node<D>> {
        match self {
            Some(node) => node.parent(),
            None => None,
        }
    }

    fn parent_ptr(&self) -> *mut Node<D> {
        match self {
            Some(node) => node.parent_ptr(),
            None => core::ptr::null_mut(),
        }
    }

    fn set_parent(&self, node: Option<&Node<D>>) {
        self.inspect(|n| n.set_parent(node));
    }

    fn left(&self) -> Option<&Node<D>> {
        match self {
            Some(node) => node.left(),
            None => None,
        }
    }

    fn left_ptr(&self) -> *mut Node<D> {
        match self {
            Some(node) => node.left_ptr(),
            None => core::ptr::null_mut(),
        }
    }

    fn set_left(&self, node: Option<&Node<D>>) {
        self.inspect(|n| n.set_left(node));
    }

    fn right(&self) -> Option<&Node<D>> {
        match self {
            Some(node) => node.right(),
            None => None,
        }
    }

    fn right_ptr(&self) -> *mut Node<D> {
        match self {
            Some(node) => node.right_ptr(),
            None => core::ptr::null_mut(),
        }
    }

    fn set_right(&self, node: Option<&Node<D>>) {
        self.inspect(|n| n.set_right(node));
    }

    fn as_mut_ptr(&self) -> *mut Node<D> {
        match self {
            Some(node) => node.as_mut_ptr(),
            None => core::ptr::null_mut(),
        }
    }
}

pub struct Node<D>
where
    D: SliceKey,
{
    pub data: D,
    color: Cell<bool>,
    parent: Cell<*mut Node<D>>,
    left: Cell<*mut Node<D>>,
    right: Cell<*mut Node<D>>,
}

impl<D> Node<D>
where
    D: SliceKey,
{
    pub fn new(data: D) -> Self {
        Node { data, color: Cell::new(RED), parent: Cell::default(), left: Cell::default(), right: Cell::default() }
    }

    pub fn height_and_balance(node: Option<&Node<D>>) -> (i32, bool) {
        match node {
            None => (0, true),
            Some(n) => {
                let (left_height, left_balance) = Self::height_and_balance(n.left());
                let (right_height, right_balance) = Self::height_and_balance(n.right());

                let height = core::cmp::max(left_height, right_height) + 1;
                let balance = left_balance && right_balance && (left_height - right_height).abs() <= 1;

                (height, balance)
            }
        }
    }

    pub fn sibling(node: &Node<D>) -> Option<&Node<D>> {
        let parent = node.parent()?;
        match node.as_mut_ptr() {
            ptr if ptr == parent.left_ptr() => parent.right(),
            ptr if ptr == parent.right_ptr() => parent.left(),
            _ => panic!("Node is not a child of its parent."),
        }
    }

    pub fn successor(node: &Node<D>) -> Option<&Node<D>> {
        let mut current = node.right()?;
        while let Some(left) = current.left() {
            current = left;
        }
        Some(current)
    }

    pub fn predecessor(node: &Node<D>) -> Option<&Node<D>> {
        let mut current = node.left()?;
        while let Some(right) = current.right() {
            current = right;
        }
        Some(current)
    }

    pub fn swap(node1: &Node<D>, node2: &Node<D>) {
        // Swap who the parent points to
        if node1.parent().left_ptr() == node1.as_mut_ptr() {
            node1.parent().set_left(Some(node2));
        } else {
            node1.parent().set_right(Some(node2));
        }

        if node2.parent().left_ptr() == node2.as_mut_ptr() {
            node2.parent().set_left(Some(node1));
        } else {
            node2.parent().set_right(Some(node1));
        }

        // Swap the colors
        let tmp_color = node1.color.get();
        node1.color.set(node2.color.get());
        node2.color.set(tmp_color);

        // Swap the parent pointers
        let tmp_parent = node1.parent.get();
        node1.parent.set(node2.parent.get());
        node2.parent.set(tmp_parent);

        // Swap the left pointers
        let tmp_left = node1.left.get();
        node1.left.set(node2.left.get());
        node2.left.set(tmp_left);

        // Swap the right pointers
        let tmp_right = node1.right.get();
        node1.right.set(node2.right.get());
        node2.right.set(tmp_right);

        // Update the parent pointers of the children
        if let Some(left) = node1.left() {
            left.set_parent(Some(node1));
        }

        if let Some(right) = node1.right() {
            right.set_parent(Some(node1));
        }

        if let Some(left) = node2.left() {
            left.set_parent(Some(node2));
        }

        if let Some(right) = node2.right() {
            right.set_parent(Some(node2));
        }
    }
}

impl<D> From<&Node<D>> for *mut Node<D>
where
    D: SliceKey,
{
    fn from(node: &Node<D>) -> *mut Node<D> {
        node.as_mut_ptr()
    }
}

impl<D: SliceKey> SliceKey for Node<D> {
    type Key = D::Key;
    fn key(&self) -> &Self::Key {
        self.data.key()
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;

    #[test]
    fn test_storage() {
        let mut memory = [0; 10 * node_size::<usize>()];
        let mut storage = Storage::<usize>::with_capacity(&mut memory);

        // Fill the storage
        for i in 0..10 {
            let (index, node) = storage.add(i).unwrap();
            assert_eq!(index, i);
            assert_eq!(node.data, i);
            assert_eq!(storage.len(), i + 1);
        }

        // Ensure we can't add more than the storage capacity
        assert!(storage.add(11).is_err());

        // Delete a node and add a new one, make sure the new one is in the same spot
        storage.delete(storage.get(5).unwrap().as_mut_ptr());
        let (index, node) = storage.add(11).unwrap();
        assert_eq!(index, 5);
        assert_eq!(node.data, 11);

        // Try and get a mutable reference to a node
        {
            let node = storage.get_mut(5).unwrap();
            assert_eq!(node.data, 11);
            node.data = 12;
        }
        let node = storage.get(5).unwrap();
        assert_eq!(node.data, 12);
    }

    #[test]
    fn test_sibling() {
        let p1 = &Node::new(1);
        let p2 = &Node::new(2);
        let p3 = &Node::new(3);
        let p4 = &Node::new(4);

        p1.set_left(Some(p2));
        p2.set_parent(Some(p1));

        p1.set_right(Some(p3));
        p3.set_parent(Some(p1));

        p4.set_parent(Some(p1));

        assert_eq!(Node::sibling(p2).unwrap().data, 3);
        assert_eq!(Node::sibling(p3).unwrap().data, 2);
        assert!(Node::sibling(p1).is_none());
    }

    #[test]
    #[should_panic = "Node is not a child of its parent."]
    fn test_sibling_panic() {
        let p1 = &Node::new(1);
        let p2 = &Node::new(2);
        let p3 = &Node::new(3);
        let p4 = &Node::new(4);

        p1.set_left(Some(p2));
        p2.set_parent(Some(p1));

        p1.set_right(Some(p3));
        p3.set_parent(Some(p1));

        p4.set_parent(Some(p1));

        Node::sibling(p4);
    }

    #[test]
    fn test_predecessor() {
        let p1 = &Node::new(1);
        let p2 = &Node::new(2);
        let p3 = &Node::new(3);
        let p4 = &Node::new(4);

        p1.set_left(Some(p2));
        p2.set_parent(Some(p1));

        p2.set_left(Some(p3));
        p3.set_parent(Some(p2));

        p2.set_right(Some(p4));
        p4.set_parent(Some(p2));

        assert_eq!(Node::predecessor(p1).unwrap().data, 4);
        assert!(Node::predecessor(p4).is_none());
    }

    #[test]
    fn test_successor() {
        let p1 = &Node::new(1);
        let p2 = &Node::new(2);
        let p3 = &Node::new(3);
        let p4 = &Node::new(4);

        p1.set_right(Some(p2));
        p2.set_parent(Some(p1));

        p2.set_left(Some(p3));
        p3.set_parent(Some(p2));

        p2.set_right(Some(p4));
        p4.set_parent(Some(p2));

        assert_eq!(Node::successor(p1).unwrap().data, 3);
        assert!(Node::successor(p4).is_none());
    }

    #[test]
    fn test_swap_works() {
        let p1 = Node::new(1);
        let p2 = Node::new(2);

        let l1 = Node::new(3);
        let l2 = Node::new(4);

        let r1 = Node::new(5);
        let r2 = Node::new(6);

        let node1 = Node::new(7);
        node1.set_red();
        let node2 = Node::new(8);
        node2.set_black();

        // Set up the tree
        node1.set_left(Some(&l1));
        l1.set_parent(Some(&node1));
        node1.set_right(Some(&r1));
        r1.set_parent(Some(&node1));
        node1.set_parent(Some(&p1));
        p1.set_left(Some(&node1));

        // set up the other tree
        node2.set_left(Some(&l2));
        l2.set_parent(Some(&node2));
        node2.set_right(Some(&r2));
        r2.set_parent(Some(&node2));
        node2.set_parent(Some(&p2));
        p2.set_right(Some(&node2));

        // Swap the nodes
        Node::swap(&node1, &node2);

        // Verify node1 is now in the place of node2
        assert!(node1.is_black());
        assert_eq!(node1.parent_ptr(), p2.as_mut_ptr());
        assert_eq!(p2.right_ptr(), node1.as_mut_ptr());
        assert_eq!(node1.left_ptr(), l2.as_mut_ptr());
        assert_eq!(l2.parent_ptr(), node1.as_mut_ptr());
        assert_eq!(node1.right_ptr(), r2.as_mut_ptr());
        assert_eq!(r2.parent_ptr(), node1.as_mut_ptr());

        // Verify node2 is now in the place of node1
        assert!(node2.is_red());
        assert_eq!(node2.parent_ptr(), p1.as_mut_ptr());
        assert_eq!(p1.left_ptr(), node2.as_mut_ptr());
        assert_eq!(node2.left_ptr(), l1.as_mut_ptr());
        assert_eq!(l1.parent_ptr(), node2.as_mut_ptr());
        assert_eq!(node2.right_ptr(), r1.as_mut_ptr());
        assert_eq!(r1.parent_ptr(), node2.as_mut_ptr());
    }
}
