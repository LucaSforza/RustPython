use crossbeam_utils::atomic::AtomicCell;
use num_traits::ToPrimitive;
use std::fmt;

use super::int::PyIntRef;
use super::pytype::PyTypeRef;
use crate::common::hash::PyHash;
use crate::pyobject::{
    self, BorrowValue, Either, IdProtocol, IntoPyObject, PyArithmaticValue, PyClassImpl,
    PyComparisonValue, PyContext, PyObjectRef, PyRef, PyResult, PyValue, TryFromObject,
    TypeProtocol,
};
use crate::sequence::{self, SimpleSeq};
use crate::sliceable::PySliceableSequence;
use crate::slots::{Comparable, Hashable, PyComparisonOp};
use crate::vm::{ReprGuard, VirtualMachine};
use crate::{bytesinner::PyBytesInner, function::OptionalArg};

/// tuple() -> empty tuple
/// tuple(iterable) -> tuple initialized from iterable's items
///
/// If the argument is a tuple, the return value is the same object.
#[pyclass(module = false, name = "tuple")]
pub struct PyTuple {
    elements: Box<[PyObjectRef]>,
}

impl fmt::Debug for PyTuple {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // TODO: implement more informational, non-recursive Debug formatter
        f.write_str("tuple")
    }
}

impl<'a> BorrowValue<'a> for PyTuple {
    type Borrowed = &'a [PyObjectRef];

    fn borrow_value(&'a self) -> Self::Borrowed {
        &self.elements
    }
}

impl PyValue for PyTuple {
    fn class(vm: &VirtualMachine) -> &PyTypeRef {
        &vm.ctx.types.tuple_type
    }
}

macro_rules! impl_intopyobj_tuple {
    ($(($T:ident, $idx:tt)),+) => {
        impl<$($T: IntoPyObject),*> IntoPyObject for ($($T,)*) {
            fn into_pyobject(self, vm: &VirtualMachine) -> PyObjectRef {
                vm.ctx.new_tuple(vec![$(self.$idx.into_pyobject(vm)),*])
            }
        }
    };
}

impl_intopyobj_tuple!((A, 0));
impl_intopyobj_tuple!((A, 0), (B, 1));
impl_intopyobj_tuple!((A, 0), (B, 1), (C, 2));
impl_intopyobj_tuple!((A, 0), (B, 1), (C, 2), (D, 3));
impl_intopyobj_tuple!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4));
impl_intopyobj_tuple!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5));
impl_intopyobj_tuple!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6));

impl PyTuple {
    pub(crate) fn fast_getitem(&self, idx: usize) -> PyObjectRef {
        self.elements[idx].clone()
    }

    // TODO: more generic way to do so
    pub(crate) fn to_bytes_inner(&self, vm: &VirtualMachine) -> PyResult<PyBytesInner> {
        let mut elements = Vec::<u8>::with_capacity(self.borrow_value().len());
        for elem in self.borrow_value().iter() {
            let py_int = PyIntRef::try_from_object(vm, elem.clone()).map_err(|_| {
                vm.new_type_error(format!(
                    "'{}' object cannot be interpreted as an integer",
                    elem.class().name
                ))
            })?;
            let result = py_int
                .borrow_value()
                .to_u8()
                .ok_or_else(|| vm.new_value_error("bytes must be in range (0, 256)".to_owned()))?;
            elements.push(result);
        }
        Ok(elements.into())
    }
}

pub type PyTupleRef = PyRef<PyTuple>;

impl PyTupleRef {
    pub(crate) fn with_elements(elements: Vec<PyObjectRef>, ctx: &PyContext) -> Self {
        if elements.is_empty() {
            ctx.empty_tuple.clone()
        } else {
            let elements = elements.into_boxed_slice();
            Self::new_ref(PyTuple { elements }, ctx.types.tuple_type.clone(), None)
        }
    }
}

pub(crate) fn get_value(obj: &PyObjectRef) -> &[PyObjectRef] {
    obj.payload::<PyTuple>().unwrap().borrow_value()
}

#[pyimpl(flags(BASETYPE), with(Hashable, Comparable))]
impl PyTuple {
    /// Creating a new tuple with given boxed slice.
    /// NOTE: for usual case, you probably want to use PyTupleRef::with_elements.
    /// Calling this function implies trying micro optimization for non-zero-sized tuple.
    pub(crate) fn _new(elements: Box<[PyObjectRef]>) -> Self {
        Self { elements }
    }

    #[pymethod(name = "__add__")]
    fn add(
        zelf: PyRef<Self>,
        other: PyObjectRef,
        vm: &VirtualMachine,
    ) -> PyArithmaticValue<PyRef<Self>> {
        let added = other.downcast::<Self>().map(|other| {
            if other.elements.is_empty() && zelf.class().is(&vm.ctx.types.tuple_type) {
                zelf
            } else if zelf.elements.is_empty() && other.class().is(&vm.ctx.types.tuple_type) {
                other
            } else {
                let elements = zelf
                    .elements
                    .boxed_iter()
                    .chain(other.borrow_value().boxed_iter())
                    .cloned()
                    .collect::<Vec<_>>()
                    .into_boxed_slice();
                Self { elements }.into_ref(vm)
            }
        });
        PyArithmaticValue::from_option(added.ok())
    }

    #[pymethod(name = "__bool__")]
    fn bool(&self) -> bool {
        !self.elements.is_empty()
    }

    #[pymethod(name = "count")]
    fn count(&self, needle: PyObjectRef, vm: &VirtualMachine) -> PyResult<usize> {
        let mut count: usize = 0;
        for element in self.elements.iter() {
            if vm.identical_or_equal(element, &needle)? {
                count += 1;
            }
        }
        Ok(count)
    }

    #[pymethod(name = "__iter__")]
    fn iter(zelf: PyRef<Self>) -> PyTupleIterator {
        PyTupleIterator {
            position: AtomicCell::new(0),
            tuple: zelf,
        }
    }

    #[pymethod(name = "__len__")]
    pub(crate) fn len(&self) -> usize {
        self.elements.len()
    }

    #[pymethod(name = "__repr__")]
    fn repr(zelf: PyRef<Self>, vm: &VirtualMachine) -> PyResult<String> {
        let s = if let Some(_guard) = ReprGuard::enter(vm, zelf.as_object()) {
            let mut str_parts = Vec::with_capacity(zelf.elements.len());
            for elem in zelf.elements.iter() {
                let s = vm.to_repr(elem)?;
                str_parts.push(s.borrow_value().to_owned());
            }

            if str_parts.len() == 1 {
                format!("({},)", str_parts[0])
            } else {
                format!("({})", str_parts.join(", "))
            }
        } else {
            "(...)".to_owned()
        };
        Ok(s)
    }

    #[pymethod(name = "__mul__")]
    #[pymethod(name = "__rmul__")]
    fn mul(&self, counter: isize, vm: &VirtualMachine) -> PyRef<Self> {
        if self.elements.is_empty() || counter == 0 {
            vm.ctx.empty_tuple.clone()
        } else {
            let elements = sequence::seq_mul(&self.elements, counter)
                .cloned()
                .collect::<Vec<_>>()
                .into_boxed_slice();
            Self { elements }.into_ref(vm)
        }
    }

    #[pymethod(name = "__getitem__")]
    fn getitem(zelf: PyRef<Self>, needle: PyObjectRef, vm: &VirtualMachine) -> PyResult {
        let result = match zelf.elements.as_ref().get_item(vm, needle, "tuple")? {
            Either::A(obj) => obj,
            Either::B(vec) => vm.ctx.new_tuple(vec),
        };
        Ok(result)
    }

    #[pymethod(name = "index")]
    fn index(&self, needle: PyObjectRef, vm: &VirtualMachine) -> PyResult<usize> {
        for (index, element) in self.elements.iter().enumerate() {
            if vm.identical_or_equal(element, &needle)? {
                return Ok(index);
            }
        }
        Err(vm.new_value_error("tuple.index(x): x not in tuple".to_owned()))
    }

    #[pymethod(name = "__contains__")]
    fn contains(&self, needle: PyObjectRef, vm: &VirtualMachine) -> PyResult<bool> {
        for element in self.elements.iter() {
            if vm.identical_or_equal(element, &needle)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    #[pyslot]
    fn tp_new(
        cls: PyTypeRef,
        iterable: OptionalArg<PyObjectRef>,
        vm: &VirtualMachine,
    ) -> PyResult<PyRef<Self>> {
        let elements = if let OptionalArg::Present(iterable) = iterable {
            let iterable = if cls.is(&vm.ctx.types.tuple_type) {
                match iterable.downcast_exact::<Self>(vm) {
                    Ok(tuple) => return Ok(tuple),
                    Err(iterable) => iterable,
                }
            } else {
                iterable
            };
            vm.extract_elements(&iterable)?
        } else {
            vec![]
        }
        .into_boxed_slice();

        Self { elements }.into_ref_with_type(vm, cls)
    }
}

impl Hashable for PyTuple {
    fn hash(zelf: &PyRef<Self>, vm: &VirtualMachine) -> PyResult<PyHash> {
        pyobject::hash_iter(zelf.elements.iter(), vm)
    }
}

impl Comparable for PyTuple {
    fn cmp(
        zelf: &PyRef<Self>,
        other: &PyObjectRef,
        op: PyComparisonOp,
        vm: &VirtualMachine,
    ) -> PyResult<PyComparisonValue> {
        if let Some(res) = op.identical_optimization(zelf, other) {
            return Ok(res.into());
        }
        let other = class_or_notimplemented!(Self, other);
        let a = zelf.borrow_value();
        let b = other.borrow_value();
        sequence::cmp(vm, a.boxed_iter(), b.boxed_iter(), op).map(PyComparisonValue::Implemented)
    }
}

#[pyclass(module = false, name = "tuple_iterator")]
#[derive(Debug)]
pub struct PyTupleIterator {
    position: AtomicCell<usize>,
    tuple: PyTupleRef,
}

impl PyValue for PyTupleIterator {
    fn class(vm: &VirtualMachine) -> &PyTypeRef {
        &vm.ctx.types.tuple_iterator_type
    }
}

#[pyimpl]
impl PyTupleIterator {
    #[pymethod(name = "__next__")]
    fn next(&self, vm: &VirtualMachine) -> PyResult {
        let pos = self.position.fetch_add(1);
        if let Some(obj) = self.tuple.borrow_value().get(pos) {
            Ok(obj.clone())
        } else {
            Err(vm.new_stop_iteration())
        }
    }

    #[pymethod(name = "__iter__")]
    fn iter(zelf: PyRef<Self>) -> PyRef<Self> {
        zelf
    }
}

pub fn init(context: &PyContext) {
    let tuple_type = &context.types.tuple_type;
    PyTuple::extend_class(context, tuple_type);

    PyTupleIterator::extend_class(context, &context.types.tuple_iterator_type);
}
