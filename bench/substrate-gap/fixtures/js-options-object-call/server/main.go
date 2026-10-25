package main

import "github.com/gin-gonic/gin"

func Login(c *gin.Context)       {}
func GetUserList(c *gin.Context) {}
func GetUserInfo(c *gin.Context) {}

func main() {
	r := gin.Default()
	base := r.Group("base")
	base.POST("login", Login)
	user := r.Group("user")
	user.POST("getUserList", GetUserList)
	user.GET("getUserInfo", GetUserInfo)
	r.Run(":8888")
}
