//! The decode of the values of a read, for the `value` bench.

#![expect(unsafe_code, reason = "a value is a `UA_DataValue` of open62541")]

use crate::ffi;
use crate::fuzz;

/// The answers of a read: decoded `UA_DataValue`s, each a Good scalar.
pub struct Values(Vec<fuzz::Value>);

impl Values {
    /// Gives `count` answers whose types are the built-in types `ids` in turn, by id in
    /// namespace 0, each with every byte of its value 1.
    ///
    /// # Panics
    ///
    /// When an id of `ids` is not that of a type from Boolean (1) to Double (11), or
    /// `ids` is empty and `count` is not 0.
    #[must_use]
    pub fn new(ids: &[u8], count: usize) -> Self {
        let data_value = &ffi::types()[ffi::DATA_VALUE];
        let answers = (0..count)
            .map(|at| {
                let id = ids[at % ids.len()];
                assert!(
                    (1..=11).contains(&id),
                    "{id} is a built-in type that a read decodes"
                );
                // The mask 0x01 encodes only the value, a scalar of the type `id`.
                let mut bytes = vec![0x01, id];
                bytes.resize(2 + ffi::types()[usize::from(id) - 1].size(), 1);
                let (answer, _) =
                    fuzz::Value::decode(data_value, &bytes).expect("a DataValue");
                answer
            })
            .collect();
        Self(answers)
    }

    /// Decodes each answer as a read does, and gives the count that decode.
    #[inline(never)]
    #[must_use]
    pub fn read(&self) -> usize {
        let mut count = 0;
        for answer in &self.0 {
            // SAFETY: each answer is a live `UA_DataValue`.
            let read = unsafe { crate::value::Value::of(answer.pointer().cast()) };
            count += usize::from(read.is_ok());
        }
        count
    }
}

impl std::fmt::Debug for Values {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Values")
            .field("count", &self.0.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::Values;

    #[test]
    fn decodes_each_answer_of_a_mix() {
        let values = Values::new(&[1, 4, 11], 5);
        assert_eq!(values.read(), 5);
        assert_eq!(format!("{values:?}"), "Values { count: 5, .. }");
    }
}
