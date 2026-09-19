import { Socket } from "phoenix"

let socket = new Socket("/socket", { params: {} })
socket.connect()

let channel = socket.channel("room:lobby", {})
channel.join()
