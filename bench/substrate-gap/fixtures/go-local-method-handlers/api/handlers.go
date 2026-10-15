package api

import "github.com/gin-gonic/gin"

type TokensHandler struct{}

func (h *TokensHandler) List(c *gin.Context)   {}
func (h *TokensHandler) Create(c *gin.Context) {}
func (h *TokensHandler) Show(c *gin.Context)   {}
func (h *TokensHandler) Del(c *gin.Context)    {}

type UsersHandler struct{}

func (u *UsersHandler) List(c *gin.Context)   {}
func (u *UsersHandler) Create(c *gin.Context) {}
func (u *UsersHandler) Show(c *gin.Context)   {}
func (u *UsersHandler) Del(c *gin.Context)    {}

func NewTokensHandler() *TokensHandler { return &TokensHandler{} }
