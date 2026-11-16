package com.acme.cache;

import com.acme.ports.OrderRepository;

public class Anonymous {
    OrderRepository make() {
        return new OrderRepository() {
            public String find() { return "anonymous"; }
        };
    }
}
