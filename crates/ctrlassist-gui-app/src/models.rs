//! Models updated in place.
//!
//! A pane's rows are made afresh from the daemon's state on every
//! change. Handing Slint a new model each time would have it destroy
//! and make again every component a `for` builds from it: the detail
//! being looked at would scroll back to its top, and the control with
//! keyboard focus would be gone, after every change the daemon reports
//! and after every change made in a form. So where the rows are the
//! same things as before, their data is replaced in the model Slint
//! already has, and the components stay.

use slint::{Model as _, ModelRc, VecModel};

/// `new` as a model: the `current` one with its rows' data replaced,
/// if it has as many rows and each is the same thing as the one taking
/// its place by `same`; a new model otherwise.
pub(crate) fn sync<T: Clone + 'static>(
    current: &ModelRc<T>,
    new: Vec<T>,
    same: impl Fn(&T, &T) -> bool,
) -> ModelRc<T> {
    if let Some(rows) = current.as_any().downcast_ref::<VecModel<T>>()
        && rows.row_count() == new.len()
        && rows.iter().zip(&new).all(|(old, new)| same(&old, new))
    {
        for (at, row) in new.into_iter().enumerate() {
            rows.set_row_data(at, row);
        }
        return current.clone();
    }
    ModelRc::new(VecModel::from(new))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_rows_keep_their_model_and_other_rows_get_a_new_one() {
        let by_first = |a: &(i32, i32), b: &(i32, i32)| a.0 == b.0;
        let empty: ModelRc<(i32, i32)> = ModelRc::default();
        let first = sync(&empty, vec![(1, 10), (2, 20)], by_first);
        assert!(first != empty);
        // The same things with other data: the model stays, the data goes in.
        let second = sync(&first, vec![(1, 11), (2, 21)], by_first);
        assert!(second == first);
        assert_eq!(second.row_data(1), Some((2, 21)));
        // Another thing in a row's place, or another count: a new model.
        assert!(sync(&second, vec![(1, 11), (3, 30)], by_first) != second);
        assert!(sync(&second, vec![(1, 11)], by_first) != second);
    }
}
