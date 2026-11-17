# A member use as a dependency

easement does not count a method, a field or a mixed-in method used through a class as a dependency on the file that defines it. A file depends on what it writes: a module, or a name at the top level of one. `may_depend_on`, `must_not_depend_on`, `must_not_reach` and `suggest` read only those.

The request comes up where a method reaches a caller through another constant. In Ruby, a concern included in a model:

```ruby
# app/repositories/order_repository.rb
module OrderRepository
  extend ActiveSupport::Concern
  class_methods do
    def admin_search(query:) = ...
  end
end

# app/models/order.rb
class Order < ApplicationRecord
  include OrderRepository
end

# app/projections/order_projection.rb
Order.admin_search(query:)
```

The projection writes `Order`, so it depends on `models`. `models` depends on `repositories` through `include OrderRepository`. `suggest` lists `projections -> repositories` as never depended on, and that is right: the projection reaches the repository only through the model.

## Why this is out of scope

ADR 0003 (`docs/adr/0003-a-dependency-is-a-written-name.md`) settled this for every language. A member use would make the verdict depend on how much each indexer infers. scip-ruby writes a reference for `Order.admin_search`, because Sorbet knows `class_methods`, but nothing for `Order.pending` when `pending` comes from a Rails `scope`, which Sorbet does not know without generated RBI files. The same policy would hold for one call and miss its neighbour. An inherited or mixed-in method would also make every caller of the model depend on the model's mixins, a dependency the caller never writes.

## What to use instead

A member use is a symbol use, and symbol rules judge it, including a method reached through a model:

```yaml
forbid_symbol:
  - name: OrderRepository::ClassMethods.admin_search
    allow: [controllers]
    because: only controllers run admin searches
```

`check` then reports `projections must not use OrderRepository::ClassMethods.admin_search (only controllers may)` at the line of `Order.admin_search`. A method Sorbet cannot see (a Rails `scope` without RBI files) is out of reach of this rule too.

## Prior requests

- #204: Ruby, a dependency through a method mixed in by an `ActiveSupport::Concern`
