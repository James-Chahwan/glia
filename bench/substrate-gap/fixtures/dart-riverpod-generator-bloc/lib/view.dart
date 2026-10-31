import 'package:flutter/material.dart';
import 'package:flutter_bloc/flutter_bloc.dart';

import 'cart_cubit.dart';

class CartView extends StatelessWidget {
  const CartView({super.key});

  @override
  Widget build(BuildContext context) {
    return TextButton(
      onPressed: () => context.read<CartCubit>().add(),
      child: const Text('add'),
    );
  }

  void reset(BuildContext context) {
    BlocProvider.of<CartCubit>(context).clear();
  }
}
