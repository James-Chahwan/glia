import 'cart_cubit.dart';

/// No provider / flutter_bloc import: `read<T>()` here is this file's own
/// extension, never a BuildContext read.
extension Lookup on Object {
  T read<T>() => throw UnimplementedError();
}

void undoAll(Object box) {
  box.read<CartCubit>().undo();
}
