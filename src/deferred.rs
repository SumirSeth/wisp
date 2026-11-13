//! Coalesced work kept outside the state borrow, including across modal loops.
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::ops::Deref;

pub struct Queue<K, V>(VecDeque<(K, V)>);
impl<K: PartialEq, V> Queue<K, V> {
    pub fn new() -> Self {
        Self(VecDeque::new())
    }
    pub fn push(&mut self, key: K, value: V) {
        // Move replacements to the back to preserve ordering with other changes.
        self.0.retain(|(old, _)| *old != key);
        self.0.push_back((key, value));
    }
    pub fn append(&mut self, key: K, value: V) {
        self.0.push_back((key, value));
    }
    pub fn pop(&mut self) -> Option<V> {
        self.0.pop_front().map(|(_, value)| value)
    }
}

type Operation<T> = Box<dyn FnOnce(&mut T)>;
pub struct Deferred<T> {
    state: RefCell<T>,
    pending: RefCell<Queue<Option<&'static str>, Operation<T>>>,
    draining: Cell<bool>,
}
impl<T> Deref for Deferred<T> {
    type Target = RefCell<T>;
    fn deref(&self) -> &Self::Target {
        &self.state
    }
}
impl<T> Deferred<T> {
    pub fn new(state: T) -> Self {
        Self {
            state: RefCell::new(state),
            pending: RefCell::new(Queue::new()),
            draining: Cell::new(false),
        }
    }
    pub fn run<R>(&self, key: Option<&'static str>, op: impl FnOnce(&mut T) -> R + 'static) -> Option<R> {
        let result = if let Ok(mut state) = self.state.try_borrow_mut() {
            Some(op(&mut state))
        } else {
            let op: Operation<T> = Box::new(move |state| {
                op(state);
            });
            if key.is_some() {
                self.pending.borrow_mut().push(key, op);
            } else {
                self.pending.borrow_mut().append(key, op);
            }
            None
        };
        self.drain();
        result
    }
    pub fn drain(&self) {
        if self.draining.replace(true) {
            return;
        }
        if let Ok(mut state) = self.state.try_borrow_mut() {
            loop {
                let next = self.pending.borrow_mut().pop();
                let Some(op) = next else {
                    break;
                };
                op(&mut state);
            }
        }
        self.draining.set(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replacement_moves_after_intervening_work() {
        let mut q = Queue::new();
        q.push("apps", 1);
        q.push("hide", 2);
        q.push("apps", 3);
        assert_eq!(q.pop(), Some(2));
        assert_eq!(q.pop(), Some(3));
        assert_eq!(q.pop(), None);
    }
    #[test]
    fn drain_accepts_reentrant_work_and_keeps_actions_in_order() {
        let cell = std::rc::Rc::new(Deferred::new(Vec::new()));
        let held = cell.borrow_mut();
        let nested = cell.clone();
        cell.run(None, move |s| {
            s.push(1);
            nested.run(None, |s| s.push(3));
        });
        cell.run(None, |s| s.push(2));
        drop(held);
        cell.drain();
        assert_eq!(*cell.borrow(), vec![1, 2, 3]);
    }
    #[test]
    fn setter_while_borrowed_defers_and_coalesces() {
        let cell = Deferred::new(Vec::new());
        let held = cell.borrow_mut();
        assert_eq!(cell.run(Some("apps"), |s| s.push(1)), None);
        cell.run(Some("apps"), |s| s.push(2));
        cell.run(Some("hide"), |s| s.push(3));
        assert!(held.is_empty());
        drop(held);
        cell.drain();
        assert_eq!(*cell.borrow(), vec![2, 3]);
    }
}
