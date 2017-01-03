// Copyright (c) 2013-2017 Sandstorm Development Group, Inc. and contributors
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
// THE SOFTWARE.

use futures::Future;

use std::cell::{Cell, RefCell};
use std::rc::{Rc};

struct ForkedPromiseInner<F> where F: Future {
    next_clone_id: Cell<u64>,
    poller: Cell<Option<u64>>,
    original_future: RefCell<F>,
    state: RefCell<ForkedPromiseState<F::Item, F::Error>>,
}

enum ForkedPromiseState<T, E> {
    Waiting(::std::collections::BTreeMap<u64, ::futures::task::Task>),
    Done(Result<T, E>),
}

pub struct ForkedPromise<F> where F: Future {
    id: u64,
    inner: Rc<ForkedPromiseInner<F>>,
}

impl <F> Clone for ForkedPromise<F> where F: Future {
    fn clone(&self) -> ForkedPromise<F> {
        let clone_id = self.inner.next_clone_id.get();
        self.inner.next_clone_id.set(clone_id + 1);
        ForkedPromise {
            id: clone_id,
            inner: self.inner.clone(),
        }
    }
}

impl <F> ForkedPromise<F> where F: Future {
    pub fn new(f: F) -> ForkedPromise<F> {
        ForkedPromise {
            id: 0,
            inner: Rc::new(ForkedPromiseInner {
                next_clone_id: Cell::new(1),
                poller: Cell::new(None),
                original_future: RefCell::new(f),
                state: RefCell::new(ForkedPromiseState::Waiting(::std::collections::BTreeMap::new())),
            })
        }
    }
}

impl<F> Drop for ForkedPromise<F> where F: Future {
    fn drop(&mut self) {
        match *self.inner.state.borrow_mut() {
            ForkedPromiseState::Waiting(ref mut waiters) => {
                match self.inner.poller.get() {
                    Some(id) => {
                        if id == self.id {
                            for (_id, waiter) in waiters {
                                waiter.unpark();
                            }
                            self.inner.poller.set(None);
                        } else {
                            waiters.remove(&self.id);
                        }
                    }
                    None => (),
                }
            }
            ForkedPromiseState::Done(_) => (),
        }
    }
}

impl <F> Future for ForkedPromise<F>
    where F: Future, F::Item: Clone, F::Error: Clone,
{
    type Item = F::Item;
    type Error = F::Error;

    fn poll(&mut self) -> ::futures::Poll<Self::Item, Self::Error> {
        match *self.inner.state.borrow_mut() {
            ForkedPromiseState::Waiting(ref mut waiters) => {
                match self.inner.poller.get() {
                    Some(id) if self.id == id => (),
                    None => self.inner.poller.set(Some(self.id)),
                    _ => {
                        waiters.insert(self.id, ::futures::task::park());
                        return Ok(::futures::Async::NotReady)
                    }
                }
            }
            ForkedPromiseState::Done(ref r) => {
                match *r {
                    Ok(ref v) => return Ok(::futures::Async::Ready(v.clone())),
                    Err(ref e) => return Err(e.clone()),
                }
            }
        };

        let done_val = match self.inner.original_future.borrow_mut().poll() {
            Ok(::futures::Async::NotReady) => {
                return Ok(::futures::Async::NotReady)
            }
            Ok(::futures::Async::Ready(v)) => Ok(v),
            Err(e) => Err(e),
        };

        match ::std::mem::replace(&mut *self.inner.state.borrow_mut(),
                                  ForkedPromiseState::Done(done_val.clone())) {
            ForkedPromiseState::Waiting(ref mut waiters) => {
                for (_id, waiter) in waiters {
                    waiter.unpark();
                }
            }
            _ => unreachable!(),
        }
        match done_val {
            Ok(v) => Ok(::futures::Async::Ready(v)),
            Err(e) => Err(e),
        }
    }
}