from rest_framework.authentication import TokenAuthentication, get_authorization_header


class TokenOrBearerAuthentication(TokenAuthentication):
    """
    Accept either:
      - Authorization: Token <key>   (DRF default)
      - Authorization: Bearer <key>  (common for agents)
    """

    def authenticate(self, request):
        auth = get_authorization_header(request).split()
        if not auth:
            return None
        if auth[0].lower() == b"bearer":
            auth = [b"token"] + auth[1:]
        if auth[0].lower() != b"token":
            return None
        if len(auth) == 1:
            return None
        if len(auth) > 2:
            return None
        return self.authenticate_credentials(auth[1].decode())

