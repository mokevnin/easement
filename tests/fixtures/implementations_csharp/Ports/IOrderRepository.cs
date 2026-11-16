namespace Acme.Ports
{
    public interface IOrderRepository
    {
        string Find();
    }

    public class InMemoryRepository : IOrderRepository
    {
        public string Find() { return "memory"; }
    }
}
