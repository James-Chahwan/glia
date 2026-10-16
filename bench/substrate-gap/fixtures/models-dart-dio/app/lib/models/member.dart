class Member {
  Member({this.memberId, this.handle, this.displayName, this.badges = const []});

  final String? memberId;
  final String? handle;
  final String? displayName;
  final List<String> badges;

  factory Member.fromJson(Map<String, dynamic> j) => Member(
        memberId: j['memberId'] as String?,
        handle: j['handle'] as String?,
        displayName: j['display_name'] as String?,
        badges: (j['badges'] as List?)?.cast<String>() ?? const [],
      );
}
