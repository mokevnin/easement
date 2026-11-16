package com.acme.cache;

import com.acme.ports.OrderRepository;

public class CachedRepo implements OrderRepository {
    public String find() { return "cached"; }
}
