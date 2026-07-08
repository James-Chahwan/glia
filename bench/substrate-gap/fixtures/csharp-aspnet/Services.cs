using System.Collections.Generic;

namespace Shop.Services
{
    public interface IUserService
    {
        User GetById(int id);
    }

    public class User
    {
        public int Id { get; set; }
        public string Name { get; set; }
    }

    public class UserService : IUserService
    {
        public User GetById(int id)
        {
            return Load(id);
        }

        private User Load(int id)
        {
            return new User { Id = id, Name = "alice" };
        }
    }
}
