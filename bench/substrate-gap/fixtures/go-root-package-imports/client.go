package rootpkg

// Client is the repository-root package's own type.
type Client struct{}

func NewClient(addr string) *Client { return &Client{} }

func (c *Client) Invoke(method string) error { return nil }
