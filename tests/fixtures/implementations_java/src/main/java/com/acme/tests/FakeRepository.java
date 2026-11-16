package com.acme.tests;

import com.acme.ports.OrderRepository;

public class FakeRepository implements OrderRepository {
    public String find() { return "fake"; }
}
