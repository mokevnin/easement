from acme.ports.repository import OrderRepository


class CachedRepo(OrderRepository):
    def find(self) -> str:
        return "cached"
