package dev.example.books;

import io.flutter.plugin.common.BasicMessageChannel;
import io.flutter.plugin.common.BinaryMessenger;
import java.util.ArrayList;
import java.util.Arrays;

public class Api {
  public static class BookApi {
    private final BinaryMessenger binaryMessenger;

    public BookApi(BinaryMessenger messenger) {
      this.binaryMessenger = messenger;
    }

    public void displayBook(Object book) {
      BasicMessageChannel<Object> channel = new BasicMessageChannel<>(
          binaryMessenger, "dev.example.BookApi.displayBook", null);
      channel.send(new ArrayList<Object>(Arrays.asList(book)), reply -> {});
    }
  }
}
