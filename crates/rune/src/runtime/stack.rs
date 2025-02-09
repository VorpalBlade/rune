use core::array;
use core::fmt;
use core::mem::replace;
use core::slice;

use crate::alloc::alloc::Global;
use crate::alloc::prelude::*;
use crate::alloc::{self, Vec};
use crate::runtime::{InstAddress, Value, VmErrorKind};

/// An error raised when accessing an address on the stack.
#[derive(Debug, PartialEq)]
#[non_exhaustive]
pub struct StackError {
    addr: InstAddress,
}

impl fmt::Display for StackError {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Tried to access out-of-bounds stack entry {}", self.addr)
    }
}

/// An error raised when accessing a slice on the stack.
#[derive(Debug, PartialEq)]
#[non_exhaustive]
pub struct SliceError {
    addr: InstAddress,
    len: usize,
    stack: usize,
}

impl fmt::Display for SliceError {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Tried to access out-of-bounds stack slice {}-{} in 0-{}",
            self.addr,
            self.addr.offset() + self.len,
            self.stack
        )
    }
}

cfg_std! {
    impl std::error::Error for StackError {}
    impl std::error::Error for SliceError {}
}

/// The stack of the virtual machine, where all values are stored.
#[derive(Default, Debug)]
pub struct Stack {
    /// The current stack of values.
    stack: Vec<Value>,
    /// The top of the current stack frame.
    ///
    /// It is not possible to interact with values below this stack frame.
    top: usize,
}

impl Stack {
    /// Construct a new stack.
    ///
    /// ```
    /// use rune::runtime::{Stack, InstAddress};
    ///
    /// let mut stack = Stack::new();
    /// stack.push(rune::to_value(String::from("Hello World"))?);
    /// let value = stack.at(InstAddress::ZERO)?;
    /// let value: String = rune::from_value(value)?;
    /// assert_eq!(value, "Hello World");
    /// # Ok::<_, rune::support::Error>(())
    /// ```
    pub const fn new() -> Self {
        Self {
            stack: Vec::new(),
            top: 0,
        }
    }

    /// The current top address of the stack.
    #[inline]
    pub const fn addr(&self) -> InstAddress {
        InstAddress::new(self.stack.len().saturating_sub(self.top))
    }

    /// Try to resize the stack with space for the given size.
    pub(crate) fn resize(&mut self, size: usize) -> alloc::Result<()> {
        if size == 0 {
            return Ok(());
        }

        let empty = Value::empty()?;
        self.stack.try_resize(self.top + size, empty)?;
        Ok(())
    }

    /// Construct a new stack with the given capacity pre-allocated.
    ///
    /// ```
    /// use rune::runtime::{Stack, InstAddress};
    ///
    /// let mut stack = Stack::with_capacity(16)?;
    /// stack.push(rune::to_value(String::from("Hello World"))?);
    /// let value = stack.at(InstAddress::ZERO)?;
    /// let value: String = rune::from_value(value)?;
    /// assert_eq!(value, "Hello World");
    /// # Ok::<_, rune::support::Error>(())
    /// ```
    pub fn with_capacity(capacity: usize) -> alloc::Result<Self> {
        Ok(Self {
            stack: Vec::try_with_capacity(capacity)?,
            top: 0,
        })
    }

    /// Check if the stack is empty.
    ///
    /// This ignores [top] and will just check if the full stack is
    /// empty.
    ///
    /// ```
    /// use rune::runtime::Stack;
    ///
    /// let mut stack = Stack::new();
    /// assert!(stack.is_empty());
    /// stack.push(rune::to_value(String::from("Hello World"))?);
    /// assert!(!stack.is_empty());
    /// # Ok::<_, rune::support::Error>(())
    /// ```
    ///
    /// [top]: Self::top()
    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }

    /// Get the length of the stack.
    ///
    /// This ignores [top] and will just return the total length of
    /// the stack.
    ///
    /// ```
    /// use rune::runtime::Stack;
    ///
    /// let mut stack = Stack::new();
    /// assert_eq!(stack.len(), 0);
    /// stack.push(rune::to_value(String::from("Hello World"))?);
    /// assert_eq!(stack.len(), 1);
    /// # Ok::<_, rune::support::Error>(())
    /// ```
    ///
    /// [top]: Self::top()
    pub fn len(&self) -> usize {
        self.stack.len()
    }

    /// Perform a raw access over the stack.
    ///
    /// This ignores [top] and will just check that the given slice
    /// index is within range.
    ///
    /// [top]: Self::top()
    pub fn get<I>(&self, index: I) -> Option<&<I as slice::SliceIndex<[Value]>>::Output>
    where
        I: slice::SliceIndex<[Value]>,
    {
        self.stack.get(index)
    }

    /// Push a value onto the stack.
    ///
    /// # Examples
    ///
    /// ```
    /// use rune::runtime::{Stack, InstAddress};
    ///
    /// let mut stack = Stack::new();
    /// stack.push(rune::to_value(String::from("Hello World"))?);
    /// let value = stack.at(InstAddress::ZERO)?;
    /// assert_eq!(rune::from_value::<String>(value)?, "Hello World");
    /// # Ok::<_, rune::support::Error>(())
    /// ```
    pub fn push<T>(&mut self, value: T) -> alloc::Result<()>
    where
        T: TryInto<Value, Error: Into<alloc::Error>>,
    {
        self.stack.try_push(value.try_into().map_err(Into::into)?)?;
        Ok(())
    }

    /// Drain the current stack down to the current stack bottom.
    pub(crate) fn drain(&mut self) -> impl DoubleEndedIterator<Item = Value> + '_ {
        self.stack.drain(self.top..)
    }

    /// Get the slice at the given address with the given length.
    ///
    /// # Examples
    ///
    /// ```
    /// use rune::runtime::{Stack, InstAddress};
    ///
    /// let mut stack = Stack::new();
    /// stack.push(rune::to_value(1i64)?);
    /// stack.push(rune::to_value(1i64)?);
    /// stack.push(rune::to_value(1i64)?);
    ///
    /// let values = stack.slice_at(InstAddress::ZERO, 2)?;
    /// assert_eq!(values.len(), 2);
    /// # Ok::<_, rune::support::Error>(())
    /// ```
    pub fn slice_at(&self, addr: InstAddress, len: usize) -> Result<&[Value], SliceError> {
        let stack_len = self.stack.len();

        if let Some(slice) = inner_slice_at(&self.stack, self.top, addr, len) {
            return Ok(slice);
        }

        Err(slice_error(stack_len, self.top, addr, len))
    }

    /// Get the mutable slice at the given address with the given length.
    pub fn slice_at_mut(
        &mut self,
        addr: InstAddress,
        len: usize,
    ) -> Result<&mut [Value], SliceError> {
        let stack_len = self.stack.len();

        if let Some(slice) = inner_slice_at_mut(&mut self.stack, self.top, addr, len) {
            return Ok(slice);
        }

        Err(slice_error(stack_len, self.top, addr, len))
    }

    /// Get the slice at the given address with the given static length.
    pub fn array_at<const N: usize>(&self, addr: InstAddress) -> Result<[&Value; N], SliceError> {
        let slice = self.slice_at(addr, N)?;
        Ok(array::from_fn(|i| &slice[i]))
    }

    /// Clear the current stack.
    pub fn clear(&mut self) {
        self.stack.clear();
        self.top = 0;
    }

    /// Iterate over the stack.
    pub fn iter(&self) -> impl Iterator<Item = &Value> + '_ {
        self.stack.iter()
    }

    /// Get the offset that corresponds to the bottom of the stack right now.
    ///
    /// The stack is partitioned into call frames, and once we enter a call
    /// frame the bottom of the stack corresponds to the bottom of the current
    /// call frame.
    pub fn top(&self) -> usize {
        self.top
    }

    /// Access the value at the given frame offset.
    ///
    /// # Examples
    ///
    /// ```
    /// use rune::runtime::{Stack, InstAddress};
    ///
    /// let mut stack = Stack::new();
    /// stack.push(rune::to_value(String::from("Hello World"))?);
    /// let value = stack.at(InstAddress::ZERO)?;
    /// let value: String = rune::from_value(value)?;
    /// assert_eq!(value, "Hello World");
    /// # Ok::<_, rune::support::Error>(())
    /// ```
    pub fn at(&self, addr: InstAddress) -> Result<&Value, StackError> {
        self.top
            .checked_add(addr.offset())
            .and_then(|n| self.stack.get(n))
            .ok_or(StackError { addr })
    }

    /// Get a value mutable at the given index from the stack bottom.
    ///
    /// # Examples
    ///
    /// ```
    /// use rune::runtime::{Stack, InstAddress};
    ///
    /// let mut stack = Stack::new();
    /// stack.push(rune::to_value(String::from("Hello World"))?);
    /// let value = stack.at(InstAddress::ZERO)?;
    /// let value: String = rune::from_value(value)?;
    /// assert_eq!(value, "Hello World");
    ///
    /// *stack.at_mut(InstAddress::ZERO)? = rune::to_value(42i64)?;
    /// let value = stack.at(InstAddress::ZERO)?;
    /// let value: i64 = rune::from_value(value)?;
    /// assert_eq!(value, 42);
    /// # Ok::<_, rune::support::Error>(())
    /// ```
    pub fn at_mut(&mut self, addr: InstAddress) -> Result<&mut Value, StackError> {
        self.top
            .checked_add(addr.offset())
            .and_then(|n| self.stack.get_mut(n))
            .ok_or(StackError { addr })
    }

    /// Swap the value at position a with the value at position b.
    pub(crate) fn swap(&mut self, a: InstAddress, b: InstAddress) -> Result<(), StackError> {
        if a == b {
            return Ok(());
        }

        let a = self
            .top
            .checked_add(a.offset())
            .filter(|&n| n < self.stack.len())
            .ok_or(StackError { addr: a })?;

        let b = self
            .top
            .checked_add(b.offset())
            .filter(|&n| n < self.stack.len())
            .ok_or(StackError { addr: b })?;

        self.stack.swap(a, b);
        Ok(())
    }

    /// Modify stack top by subtracting the given count from it while checking
    /// that it is in bounds of the stack.
    ///
    /// This is used internally when returning from a call frame.
    ///
    /// Returns the old stack top.
    #[tracing::instrument(skip_all)]
    pub(crate) fn swap_top(&mut self, addr: InstAddress, len: usize) -> Result<usize, VmErrorKind> {
        let old_len = self.stack.len();

        if len == 0 {
            return Ok(replace(&mut self.top, old_len));
        }

        let Some(start) = self.top.checked_add(addr.offset()) else {
            return Err(VmErrorKind::StackError {
                error: StackError { addr },
            });
        };

        let Some(new_len) = old_len.checked_add(len) else {
            return Err(VmErrorKind::StackError {
                error: StackError { addr },
            });
        };

        if old_len < start + len {
            return Err(VmErrorKind::StackError {
                error: StackError { addr },
            });
        }

        self.stack.try_reserve(len)?;

        // SAFETY: We've ensured that the collection has space for the new
        // values. It is also guaranteed to be non-overlapping.
        unsafe {
            let ptr = self.stack.as_mut_ptr();
            let from = slice::from_raw_parts(ptr.add(start), len);

            for (value, n) in from.iter().zip(old_len..) {
                ptr.add(n).write(value.clone());
            }

            self.stack.set_len(new_len);
        }

        Ok(replace(&mut self.top, old_len))
    }

    /// Pop the current stack top and modify it to a different one.
    ///
    /// This asserts that the size of the current stack frame is exactly zero
    /// before restoring it.
    #[tracing::instrument(skip_all)]
    pub(crate) fn pop_stack_top(&mut self, top: usize) -> alloc::Result<()> {
        tracing::trace!(stack = self.stack.len(), self.top);
        self.stack.truncate(self.top);
        self.top = top;
        Ok(())
    }
}

#[inline(always)]
fn inner_slice_at(values: &[Value], top: usize, addr: InstAddress, len: usize) -> Option<&[Value]> {
    if len == 0 {
        return Some(&[]);
    }

    let start = top.checked_add(addr.offset())?;
    let end = start.checked_add(len)?;
    values.get(start..end)
}

#[inline(always)]
fn inner_slice_at_mut(
    values: &mut [Value],
    top: usize,
    addr: InstAddress,
    len: usize,
) -> Option<&mut [Value]> {
    if len == 0 {
        return Some(&mut []);
    }

    let start = top.checked_add(addr.offset())?;
    let end = start.checked_add(len)?;
    values.get_mut(start..end)
}

#[inline(always)]
fn slice_error(stack: usize, bottom: usize, addr: InstAddress, len: usize) -> SliceError {
    SliceError {
        addr,
        len,
        stack: stack.saturating_sub(bottom),
    }
}

impl TryClone for Stack {
    fn try_clone(&self) -> alloc::Result<Self> {
        Ok(Self {
            stack: self.stack.try_clone()?,
            top: self.top,
        })
    }
}

impl TryFromIteratorIn<Value, Global> for Stack {
    fn try_from_iter_in<T: IntoIterator<Item = Value>>(
        iter: T,
        alloc: Global,
    ) -> alloc::Result<Self> {
        Ok(Self {
            stack: iter.into_iter().try_collect_in(alloc)?,
            top: 0,
        })
    }
}

impl From<Vec<Value>> for Stack {
    fn from(stack: Vec<Value>) -> Self {
        Self { stack, top: 0 }
    }
}
