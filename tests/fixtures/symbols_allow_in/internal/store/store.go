package store

// Client talks to the database.
type Client struct{}

// Open opens a client; a use inside the defining file is not a use.
func Open() *Client {
	return &Client{}
}
