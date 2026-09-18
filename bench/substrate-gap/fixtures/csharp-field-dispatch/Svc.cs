using Shop.Data;

namespace Shop.Services
{
    public class UserService
    {
        private readonly UserRepo _repo;

        public UserRepo Archive { get; }

        public UserService(UserRepo repo)
        {
            _repo = repo;
            Archive = repo;
        }

        public string Get(int id)
        {
            return _repo.Find(id);
        }

        public string Lookup(int id)
        {
            return this._repo.Find(id);
        }

        public string Restore(int id)
        {
            return Archive.Find(id);
        }
    }
}
