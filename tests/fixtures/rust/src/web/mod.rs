use std::fmt;

use crate::domain::order::Order;
use crate::infra::repo::Repo;

pub fn show(_: &Repo, _: &Order, _: &dyn fmt::Debug) {}

#[cfg(test)]
mod tests {
    use super::*;
}
