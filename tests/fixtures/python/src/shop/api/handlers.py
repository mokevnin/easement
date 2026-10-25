from shop.domain import order
from shop.infra.db import save
from . import views


def create():
    return save(order.Order(id="1"))
