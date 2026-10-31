import 'package:flutter/foundation.dart';

class PromptViewModel extends ChangeNotifier {
  void addBasicIngredients(Set<String> items) {}

  void addCategoryFilters(Set<String> items) {}

  bool get ready => true;
}

/// A look-alike with the same method names.
class SavedViewModel extends ChangeNotifier {
  void addBasicIngredients(Set<String> items) {}

  void addCategoryFilters(Set<String> items) {}
}
