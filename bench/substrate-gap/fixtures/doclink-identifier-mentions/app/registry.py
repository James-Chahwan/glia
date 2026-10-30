class ServiceRegistry:
    def lookup(self, name):
        return name

def lookup_service(name):
    return ServiceRegistry().lookup(name)
