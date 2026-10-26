package handlers

import (
	"net/http"

	"github.com/gin-gonic/gin"

	"example.com/inbox/api/models"
)

func GetMessage(c *gin.Context) {
	c.JSON(http.StatusOK, models.Message{MessageID: c.Param("id"), Subject: "hello"})
}

func ListMessages(c *gin.Context) {
	c.JSON(http.StatusOK, []models.Message{})
}
