// Licensed to the Apache Software Foundation (ASF) under one
// or more contributor license agreements.  See the NOTICE file
// distributed with this work for additional information
// regarding copyright ownership.  The ASF licenses this file
// to you under the Apache License, Version 2.0 (the
// "License"); you may not use this file except in compliance
// with the License.  You may obtain a copy of the License at
//
//   http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing,
// software distributed under the License is distributed on an
// "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied.  See the License for the
// specific language governing permissions and limitations
// under the License.

//! Negation (-) expression

use crate::intervals::Interval;
use crate::physical_expr::down_cast_any_ref;
use crate::sort_properties::SortProperties;
use crate::PhysicalExpr;
use arrow::{
    compute::kernels::numeric::neg_wrapping,
    datatypes::{DataType, Schema},
    record_batch::RecordBatch,
};
use datafusion_common::{internal_err, DataFusionError, Result};
use datafusion_expr::{
    type_coercion::{is_interval, is_null, is_signed_numeric},
    ColumnarValue,
};

use std::any::Any;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// Negative expression
#[derive(Debug, Hash)]
pub struct NegativeExpr {
    /// Input expression
    arg: Arc<dyn PhysicalExpr>,
}

impl NegativeExpr {
    /// Create new not expression
    pub fn new(arg: Arc<dyn PhysicalExpr>) -> Self {
        Self { arg }
    }

    /// Get the input expression
    pub fn arg(&self) -> &Arc<dyn PhysicalExpr> {
        &self.arg
    }
}

impl std::fmt::Display for NegativeExpr {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "(- {})", self.arg)
    }
}

impl PhysicalExpr for NegativeExpr {
    /// Return a reference to Any that can be used for downcasting
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn data_type(&self, input_schema: &Schema) -> Result<DataType> {
        self.arg.data_type(input_schema)
    }

    fn nullable(&self, input_schema: &Schema) -> Result<bool> {
        self.arg.nullable(input_schema)
    }

    fn evaluate(&self, batch: &RecordBatch) -> Result<ColumnarValue> {
        let arg = self.arg.evaluate(batch)?;
        match arg {
            ColumnarValue::Array(array) => {
                let result = neg_wrapping(array.as_ref())?;
                Ok(ColumnarValue::Array(result))
            }
            ColumnarValue::Scalar(scalar) => {
                Ok(ColumnarValue::Scalar((scalar.arithmetic_negate())?))
            }
        }
    }

    fn children(&self) -> Vec<Arc<dyn PhysicalExpr>> {
        vec![self.arg.clone()]
    }

    fn with_new_children(
        self: Arc<Self>,
        children: Vec<Arc<dyn PhysicalExpr>>,
    ) -> Result<Arc<dyn PhysicalExpr>> {
        Ok(Arc::new(NegativeExpr::new(children[0].clone())))
    }

    fn dyn_hash(&self, state: &mut dyn Hasher) {
        let mut s = state;
        self.hash(&mut s);
    }

    /// Given the child interval of a NegativeExpr, it calculates the NegativeExpr's interval.
    /// It replaces the upper and lower bounds after multiplying them with -1.
    /// Ex: `(a, b]` => `[-b, -a)`
    fn evaluate_bounds(&self, children: &[&Interval]) -> Result<Interval> {
        Ok(Interval::new(
            children[0].upper.negate()?,
            children[0].lower.negate()?,
        ))
    }

    /// Returns a new [`Interval`] of a NegativeExpr  that has the existing `interval` given that
    /// given the input interval is known to be `children`.
    fn propagate_constraints(
        &self,
        interval: &Interval,
        children: &[&Interval],
    ) -> Result<Vec<Option<Interval>>> {
        let child_interval = children[0];
        let negated_interval =
            Interval::new(interval.upper.negate()?, interval.lower.negate()?);

        Ok(vec![child_interval.intersect(negated_interval)?])
    }

    /// The ordering of a [`NegativeExpr`] is simply the reverse of its child.
    fn get_ordering(&self, children: &[SortProperties]) -> SortProperties {
        -children[0]
    }
}

impl PartialEq<dyn Any> for NegativeExpr {
    fn eq(&self, other: &dyn Any) -> bool {
        down_cast_any_ref(other)
            .downcast_ref::<Self>()
            .map(|x| self.arg.eq(&x.arg))
            .unwrap_or(false)
    }
}

/// Creates a unary expression NEGATIVE
///
/// # Errors
///
/// This function errors when the argument's type is not signed numeric
pub fn negative(
    arg: Arc<dyn PhysicalExpr>,
    input_schema: &Schema,
) -> Result<Arc<dyn PhysicalExpr>> {
    let data_type = arg.data_type(input_schema)?;
    if is_null(&data_type) {
        Ok(arg)
    } else if !is_signed_numeric(&data_type) && !is_interval(&data_type) {
        internal_err!(
            "Can't create negative physical expr for (- '{arg:?}'), the type of child expr is {data_type}, not signed numeric"
        )
    } else {
        Ok(Arc::new(NegativeExpr::new(arg)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        expressions::{col, Column},
        intervals::Interval,
    };
    use arrow::array::*;
    use arrow::datatypes::*;
    use arrow_schema::DataType::{Float32, Float64, Int16, Int32, Int64, Int8};
    use datafusion_common::{cast::as_primitive_array, Result};
    use paste::paste;

    macro_rules! test_array_negative_op {
        ($DATA_TY:tt, $($VALUE:expr),*   ) => {
            let schema = Schema::new(vec![Field::new("a", DataType::$DATA_TY, true)]);
            let expr = negative(col("a", &schema)?, &schema)?;
            assert_eq!(expr.data_type(&schema)?, DataType::$DATA_TY);
            assert!(expr.nullable(&schema)?);
            let mut arr = Vec::new();
            let mut arr_expected = Vec::new();
            $(
                arr.push(Some($VALUE));
                arr_expected.push(Some(-$VALUE));
            )+
            arr.push(None);
            arr_expected.push(None);
            let input = paste!{[<$DATA_TY Array>]::from(arr)};
            let expected = &paste!{[<$DATA_TY Array>]::from(arr_expected)};
            let batch =
                RecordBatch::try_new(Arc::new(schema.clone()), vec![Arc::new(input)])?;
            let result = expr.evaluate(&batch)?.into_array(batch.num_rows());
            let result =
                as_primitive_array(&result).expect(format!("failed to downcast to {:?}Array", $DATA_TY).as_str());
            assert_eq!(result, expected);
        };
    }

    #[test]
    fn array_negative_op() -> Result<()> {
        test_array_negative_op!(Int8, 2i8, 1i8);
        test_array_negative_op!(Int16, 234i16, 123i16);
        test_array_negative_op!(Int32, 2345i32, 1234i32);
        test_array_negative_op!(Int64, 23456i64, 12345i64);
        test_array_negative_op!(Float32, 2345.0f32, 1234.0f32);
        test_array_negative_op!(Float64, 23456.0f64, 12345.0f64);
        Ok(())
    }

    #[test]
    fn test_evaluate_bounds() -> Result<()> {
        let negative_expr = NegativeExpr {
            arg: Arc::new(Column::new("a", 0)),
        };
        let child_interval = Interval::make(Some(-2), Some(1), (true, false));
        let negative_expr_interval = Interval::make(Some(-1), Some(2), (false, true));
        assert_eq!(
            negative_expr.evaluate_bounds(&[&child_interval])?,
            negative_expr_interval
        );
        Ok(())
    }

    #[test]
    fn test_propagate_constraints() -> Result<()> {
        let negative_expr = NegativeExpr {
            arg: Arc::new(Column::new("a", 0)),
        };
        let original_child_interval = Interval::make(Some(-2), Some(3), (false, false));
        let negative_expr_interval = Interval::make(Some(0), Some(4), (true, false));
        let after_propagation =
            vec![Some(Interval::make(Some(-2), Some(0), (false, true)))];
        assert_eq!(
            negative_expr.propagate_constraints(
                &negative_expr_interval,
                &[&original_child_interval]
            )?,
            after_propagation
        );
        Ok(())
    }
}