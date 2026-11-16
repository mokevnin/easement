package com.acme.adapters;

import com.acme.ports.OrderRepository;

public class JdbcRepository implements OrderRepository {
    public String find() { return "jdbc"; }
}
