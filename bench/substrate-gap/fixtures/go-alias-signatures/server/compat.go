package server

import "example.com/aliases/pb"

// LegacyStream: an alias of an alias, in another package.
type LegacyStream = pb.Chat_StreamServer

type ChatLegacy struct{}

func (c ChatLegacy) Stream(s LegacyStream) error { return nil }

func (c ChatLegacy) Send(m *pb.Msg) (*pb.Reply, error) { return nil, nil }
