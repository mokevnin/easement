using Acme.Ports;

namespace Acme.Tests
{
    public class FakeRepository : IOrderRepository
    {
        public string Find() { return "fake"; }
    }
}
