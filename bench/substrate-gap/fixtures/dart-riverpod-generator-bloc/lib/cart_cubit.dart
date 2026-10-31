import 'package:flutter_bloc/flutter_bloc.dart';

class CartCubit extends Cubit<int> {
  CartCubit() : super(0);

  void add() => emit(state + 1);

  void clear() => emit(0);

  void undo() => emit(state - 1);
}

/// A look-alike: same method names, never a context read's type here.
class WishCubit extends Cubit<int> {
  WishCubit() : super(0);

  void add() => emit(state + 1);

  void clear() => emit(0);
}
