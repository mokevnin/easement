from abc import ABC, abstractmethod


class OrderRepository(ABC):
    @abstractmethod
    def find(self) -> str: ...


class InMemoryRepository(OrderRepository):
    def find(self) -> str:
        return "memory"
