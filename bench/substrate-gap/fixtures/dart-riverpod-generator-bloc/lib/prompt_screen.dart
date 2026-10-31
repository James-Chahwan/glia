import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import 'prompt_view_model.dart';

class PromptScreen extends StatelessWidget {
  const PromptScreen({super.key});

  @override
  Widget build(BuildContext context) {
    final viewModel = context.watch<PromptViewModel>();
    viewModel.addBasicIngredients({'salt'});
    return const Text('prompt');
  }

  void filter(BuildContext context) {
    context.watch<PromptViewModel?>()?.addCategoryFilters({'vegan'});
  }

  bool ready(BuildContext context) {
    return context.read<PromptViewModel>().ready;
  }
}
