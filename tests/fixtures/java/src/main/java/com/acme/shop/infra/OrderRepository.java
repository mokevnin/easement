package com.acme.shop.infra;

import com.acme.shop.domain.Order;

public class OrderRepository {
    public Order find(String id) { return new Order(id); }
}
