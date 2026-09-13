package com.dhsdevelopments.kap.jvmmod;

public class Foo {
    public String standardTest() {
        return "some result";
    }

    public static String staticTest(String arg) {
        return "message from method: " + arg;
    }

    public static String throwExceptionsTest() {
        throw new RuntimeException("Test exception");
    }
}