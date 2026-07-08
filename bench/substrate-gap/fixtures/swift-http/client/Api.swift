import Foundation

// Swift URLSession client -> backend route in separate server dir.
func fetchUser(id: String) {
    let url = URL(string: "https://api.example.com/users/\(id)")!
    let task = URLSession.shared.dataTask(with: url) { data, response, error in
        // handle response
    }
    task.resume()
}

func createUser(body: Data) {
    let url = URL(string: "https://api.example.com/users")!
    var request = URLRequest(url: url)
    request.httpMethod = "POST"
    request.httpBody = body
    let task = URLSession.shared.dataTask(with: request) { data, response, error in
    }
    task.resume()
}
