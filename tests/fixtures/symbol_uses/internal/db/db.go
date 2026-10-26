package db

type Client struct{}

func (c *Client) Query() {}

func Scoped(id int64) *Client { return &Client{} }

func Tx() {}
