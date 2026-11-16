using Acme.Ports;

namespace Acme.Cache
{
    public class CachedRepo : IOrderRepository
    {
        public string Find() { return "cached"; }
    }

    public interface IReadOnly : IOrderRepository
    {
    }
}
