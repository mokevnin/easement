package com.acme.shop.web;

import java.util.List;
import static com.acme.shop.domain.Order.empty;
import com.acme.shop.infra.*;

public class OrderController {
    List<Object> all() { return List.of(empty(), new OrderRepository()); }
}
