package fx

type OXR struct{ id string }

func NewOXR(id string) *OXR { return &OXR{id: id} }

func (o *OXR) Provider() string { return "oxr" }

type Other struct{}

func NewOther() *Other { return &Other{} }

func (o *Other) Provider() string { return "other" }
