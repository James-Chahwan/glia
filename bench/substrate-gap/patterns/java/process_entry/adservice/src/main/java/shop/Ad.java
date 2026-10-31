package shop;

public final class Ad {
    private Ad() {}

    public static void main(String[] args) throws InterruptedException {
        System.out.println("ad service started");
        Thread.currentThread().join();
    }
}
