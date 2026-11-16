from acme.ports.repository import OrderRepository


class FakeRepository(OrderRepository):
    def find(self) -> str:
        return "fake"
