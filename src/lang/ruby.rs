//! Ruby beyond what every language shares: how scip-ruby's symbols read.

use crate::scip::symbol::{
    Called, Kind, Member, Name, Package, Parsed, Qualifier, Reading, Shape, Through, cut, leading,
    names,
};

/// Ruby: modules and classes are both types, and a constant names one; everything
/// below it is a member (`Shop#Web#Order#run().`). A class method belongs to the
/// singleton class (``Shop#Web#`<Class:Order>`#find().``), which stands for the class.
pub fn read(symbol: &str, parsed: &Parsed<'_>) -> Name {
    let names = names(parsed);
    let types = leading(parsed, 0, Kind::Type);
    let mut path = names[..types].to_vec();
    let mut class = cut(symbol, types);
    if let Some(last) = path.last_mut()
        && let Some(singleton) = last
            .strip_prefix("<Class:")
            .and_then(|name| name.strip_suffix('>'))
            .map(str::to_string)
    {
        class = format!("{}{singleton}#", cut(symbol, types - 1));
        *last = singleton;
    }
    let inner = names[types..].to_vec();
    let shape = match (types, inner.is_empty()) {
        (0, _) => Shape::Package,
        (_, true) => Shape::Module,
        (_, false) => Shape::Member,
    };
    let qualified = path.join("::");
    Name {
        shape,
        symbol: match shape {
            Shape::Module => class.clone(),
            _ => symbol.to_string(),
        },
        owner: (shape == Shape::Member).then_some(class),
        package: Package::Credited(parsed.owner()),
        called: path.split_last().map(|(name, outer)| Called {
            qualifier: Qualifier::Spelled(outer.join("::")),
            name: name.clone(),
            member: inner.first().map(|member| Member {
                through: Through::Spelled(qualified.clone()),
                name: member.clone(),
            }),
        }),
        external: qualified,
        path,
        inner,
        reading: Reading {
            enclosing: true,
            ..Reading::default()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scip::symbol::split;

    fn read_ruby(symbol: &str) -> Name {
        read(symbol, &split(symbol).unwrap())
    }

    /// A class method belongs to the singleton class, which stands for the class: the
    /// singleton named on its own is the class's module, and a method of it is a
    /// member owned by the class.
    #[test]
    fn a_singleton_class_stands_for_its_class() {
        let singleton = read_ruby("scip-ruby gem shop 1.0 Shop#Web#`<Class:Order>`#");
        assert_eq!(singleton.shape, Shape::Module);
        assert_eq!(singleton.symbol, "scip-ruby gem shop 1.0 Shop#Web#Order#");
        assert_eq!(singleton.owner, None);
        let method = read_ruby("scip-ruby gem shop 1.0 Shop#Web#`<Class:Order>`#find().");
        assert_eq!(method.shape, Shape::Member);
        assert_eq!(
            method.symbol,
            "scip-ruby gem shop 1.0 Shop#Web#`<Class:Order>`#find()."
        );
        assert_eq!(
            method.owner.as_deref(),
            Some("scip-ruby gem shop 1.0 Shop#Web#Order#")
        );
        assert_eq!(method.path, ["Shop", "Web", "Order"]);
        assert_eq!(method.inner, ["find"]);
    }
}
